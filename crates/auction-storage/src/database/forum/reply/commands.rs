//! 论坛回帖竞价命令账本的幂等开始与完成写入。

use crate::database::storage_error_from;

use auction_application::core::ApplicationError;
use auction_application::forum::{ForumBidCommandRecord, ForumCommandStart, ForumCommandStatus};
use uuid::Uuid;

use super::super::super::DatabaseStore;
use super::validation::storage_error;

pub(super) async fn begin_command_postgres(
    store: &DatabaseStore,
    record: ForumBidCommandRecord,
) -> Result<ForumCommandStart, ApplicationError> {
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启论坛命令账本事务失败"))?;
    // 与生命周期锁串行：扫描中并发进入 Settling/Settled 后不创建或重试论坛命令。
    // 竞价应用事务仍独立复核状态；这里不建立第二套状态机。
    let open = sqlx::query_scalar::<_, bool>(
        "SELECT status IN ('live', 'final') FROM public.auctions WHERE id = $1 FOR SHARE",
    )
    .bind(record.auction_id.as_uuid())
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定论坛命令接收状态失败"))?;
    if !open {
        return Err(storage_error("拍卖已停止接收论坛命令"));
    }
    let inserted = sqlx::query(
        "INSERT INTO public.auction_forum_bid_commands
            (command_id, auction_id, world_id, thread_id, post_id, post_number, line_number,
             lot_code, actor, command_kind, amount, status, attempt_count)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'pending', 1)
         ON CONFLICT DO NOTHING",
    )
    .bind(record.command_id)
    .bind(record.auction_id.as_uuid())
    .bind(record.world.as_str())
    .bind(&record.thread_id)
    .bind(&record.post_id)
    .bind(record.post_number)
    .bind(record.line_number)
    .bind(&record.lot_code)
    .bind(&record.actor)
    .bind(&record.command_kind)
    .bind(record.amount)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入论坛命令账本失败"))?;
    if inserted.rows_affected() == 1 {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交论坛命令账本事务失败"))?;
        return Ok(ForumCommandStart {
            command_id: record.command_id,
            status: ForumCommandStatus::Pending,
            should_run: true,
            is_new: true,
        });
    }

    // canonical v28 旧账本使用另一套稳定 ID；自然键相同且负载逐项一致时必须复用旧 ID，
    // 否则历史回复会在每轮全主题扫描中持续触发唯一约束并阻断后续新命令。
    let rows = sqlx::query(
        "SELECT command_id, auction_id, world_id, thread_id, post_id, line_number, lot_code,
                actor, command_kind, amount, status
         FROM public.auction_forum_bid_commands
         WHERE command_id = $1
            OR (auction_id = $2
                AND thread_id = $3
                AND post_id = $4
                AND line_number = $5
                AND lot_code = $6
                AND command_kind = $7)
         FOR UPDATE",
    )
    .bind(record.command_id)
    .bind(record.auction_id.as_uuid())
    .bind(&record.thread_id)
    .bind(&record.post_id)
    .bind(record.line_number)
    .bind(&record.lot_code)
    .bind(&record.command_kind)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取既有论坛命令账本失败"))?;
    let [row] = rows.as_slice() else {
        return Err(storage_error("论坛命令账本冲突记录不存在或不唯一"));
    };
    let existing_command_id: Uuid = row
        .try_get("command_id")
        .map_err(|error| storage_error_from(error, "读取论坛命令标识失败"))?;
    let existing_auction_id: Uuid = row
        .try_get("auction_id")
        .map_err(|error| storage_error_from(error, "读取论坛命令拍卖标识失败"))?;
    let existing_world: String = row
        .try_get("world_id")
        .map_err(|error| storage_error_from(error, "读取论坛命令世界失败"))?;
    let existing_thread_id: String = row
        .try_get("thread_id")
        .map_err(|error| storage_error_from(error, "读取论坛命令主题标识失败"))?;
    let existing_post_id: String = row
        .try_get("post_id")
        .map_err(|error| storage_error_from(error, "读取论坛命令帖子标识失败"))?;
    let existing_line_number: i32 = row
        .try_get("line_number")
        .map_err(|error| storage_error_from(error, "读取论坛命令行号失败"))?;
    let existing_lot_code: String = row
        .try_get("lot_code")
        .map_err(|error| storage_error_from(error, "读取论坛命令 lot 编码失败"))?;
    let existing_actor: String = row
        .try_get("actor")
        .map_err(|error| storage_error_from(error, "读取论坛命令操作者失败"))?;
    let existing_kind: String = row
        .try_get("command_kind")
        .map_err(|error| storage_error_from(error, "读取论坛命令类型失败"))?;
    let existing_amount: i64 =
        row.try_get("amount").map_err(|error| storage_error_from(error, "读取论坛命令金额失败"))?;
    let stored_status: String =
        row.try_get("status").map_err(|error| storage_error_from(error, "读取论坛命令状态失败"))?;
    // post_id 与原业务负载决定身份；全主题重新编号不是负载变化，不能阻断已完成命令。
    let same = existing_auction_id == record.auction_id.as_uuid()
        && existing_world == record.world.as_str()
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
    // 所有既有楼层都保留首次观察，旧账本的 NULL 也不能用删帖后的当前编号补造。
    let (status, should_run) = match stored_status.as_str() {
        "pending" => {
            sqlx::query(
                "UPDATE public.auction_forum_bid_commands
                 SET attempt_count = attempt_count + 1, updated_at = CURRENT_TIMESTAMP
                 WHERE command_id = $1",
            )
            .bind(existing_command_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| storage_error_from(error, "更新论坛命令重试次数失败"))?;
            (ForumCommandStatus::Pending, true)
        }
        "applied" => (ForumCommandStatus::Applied, false),
        "rejected" => (ForumCommandStatus::Rejected, false),
        _ => return Err(storage_error("论坛命令账本状态不在闭集内")),
    };
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交既有论坛命令账本事务失败"))?;
    Ok(ForumCommandStart { command_id: existing_command_id, status, should_run, is_new: false })
}

pub(super) async fn finish_command_postgres(
    store: &DatabaseStore,
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
    .map_err(|error| storage_error_from(error, "完成论坛命令账本失败"))?;
    if result.rows_affected() == 1 {
        return Ok(());
    }
    let current: Option<String> =
        sqlx::query("SELECT status FROM public.auction_forum_bid_commands WHERE command_id = $1")
            .bind(command_id)
            .fetch_optional(store.pool())
            .await
            .map_err(|error| storage_error_from(error, "读取论坛命令完成状态失败"))?
            .map(|row| row.try_get("status"))
            .transpose()
            .map_err(|error| storage_error_from(error, "解析论坛命令完成状态失败"))?;
    match current.as_deref() {
        Some(current) if current == status => Ok(()),
        Some(_) => Err(storage_error("论坛命令已由其他状态完成")),
        None => Err(storage_error("论坛命令账本记录不存在")),
    }
}
