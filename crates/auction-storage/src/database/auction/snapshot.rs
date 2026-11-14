//! `SettlementAuctionSnapshot` 的唯一行投影：Admin 拍卖列表、Admin lot 页、结算报告与排期修改
//! 共用列清单与解码，新增字段只在这里同步。

use auction_application::settlement::SettlementAuctionSnapshot;
use auction_domain::AuctionId;
use sqlx::postgres::PgRow;
use sqlx::{Decode, Postgres, Row, Type};

/// 快照列清单；调用方 FROM 必须把 `public.auctions` 别名为 `a`，列名与 [`decode_auction_snapshot`]
/// 读取的名字一致。
macro_rules! auction_snapshot_columns {
    () => {
        "a.id, a.world_id, a.sequence, a.status, a.preview_scheduled_at, a.scheduled_open_at,
         a.preview_opened_at, a.preview_manual_open, a.preview_revision, a.opened_at,
         a.final_phase_at, a.settled_at, a.cancelled_at, a.cancelled_reason, a.cancelled_by,
         a.state_version, a.scheduled_end_at, a.effective_end_at, a.created_at, a.updated_at"
    };
}

pub(crate) use auction_snapshot_columns;

/// 解码快照列；失败时附带该列的固定诊断上下文，调用方按自身错误类型映射。
pub(crate) fn decode_auction_snapshot(
    row: &PgRow,
) -> Result<SettlementAuctionSnapshot, (&'static str, sqlx::Error)> {
    Ok(SettlementAuctionSnapshot {
        id: AuctionId::from_uuid(column(row, "id", "读取拍卖快照标识失败")?),
        world_id: column(row, "world_id", "读取拍卖快照世界失败")?,
        sequence: column(row, "sequence", "读取拍卖快照序号失败")?,
        status: column(row, "status", "读取拍卖快照状态失败")?,
        preview_scheduled_at: column(row, "preview_scheduled_at", "读取拍卖快照预览计划时间失败")?,
        scheduled_open_at: column(row, "scheduled_open_at", "读取拍卖快照开放计划时间失败")?,
        preview_opened_at: column(row, "preview_opened_at", "读取拍卖快照预览开放时间失败")?,
        preview_manual_open: column(row, "preview_manual_open", "读取拍卖快照手动开放标记失败")?,
        preview_revision: column(row, "preview_revision", "读取拍卖快照预览版本失败")?,
        opened_at: column(row, "opened_at", "读取拍卖快照开放时间失败")?,
        final_phase_at: column(row, "final_phase_at", "读取拍卖快照最终阶段时间失败")?,
        settled_at: column(row, "settled_at", "读取拍卖快照结算时间失败")?,
        cancelled_at: column(row, "cancelled_at", "读取拍卖快照取消时间失败")?,
        cancelled_reason: column(row, "cancelled_reason", "读取拍卖快照取消原因失败")?,
        cancelled_by: column(row, "cancelled_by", "读取拍卖快照取消操作者失败")?,
        state_version: column(row, "state_version", "读取拍卖快照状态版本失败")?,
        scheduled_end_at: column(row, "scheduled_end_at", "读取拍卖快照计划结束时间失败")?,
        effective_end_at: column(row, "effective_end_at", "读取拍卖快照有效结束时间失败")?,
        created_at: column(row, "created_at", "读取拍卖快照创建时间失败")?,
        updated_at: column(row, "updated_at", "读取拍卖快照更新时间失败")?,
    })
}

fn column<'r, T>(
    row: &'r PgRow,
    name: &'static str,
    context: &'static str,
) -> Result<T, (&'static str, sqlx::Error)>
where
    T: Decode<'r, Postgres> + Type<Postgres>,
{
    row.try_get(name).map_err(|error| (context, error))
}
