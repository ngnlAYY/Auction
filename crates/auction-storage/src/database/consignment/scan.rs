//! Worker 待办扫描的键集分页读取辅助：游标绑定与下一页游标判定。
//!
//! 保护不变量 5：游标只是本次扫描内的读取位置，不持久、不跨轮、不作为事实来源；读不满一页即扫描结束，
//! 被来源跳过的行同样计入游标，异常行不会占住后面的待办。

use auction_application::consignment::{ScanCursor, ScanPage};
use auction_time::Timestamp;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

/// 把游标拆成两个可空绑定值（排序时刻、主键）；没有游标即从头开始。
pub(crate) fn cursor_binds(after: Option<ScanCursor>) -> (Option<Timestamp>, Option<Uuid>) {
    (after.map(|cursor| cursor.sort_at), after.map(|cursor| cursor.tie_id))
}

/// 读出待办行的排序键；查询须把排序列与主键分别别名为 `scan_at`、`scan_id`。
pub(crate) fn row_cursor(row: &PgRow) -> Result<ScanCursor, sqlx::Error> {
    Ok(ScanCursor { sort_at: row.try_get("scan_at")?, tie_id: row.try_get("scan_id")? })
}

/// 把每一行映射成一项并附上下一页游标；映射失败即整页失败。
pub(crate) fn page_of<T>(
    rows: &[PgRow],
    limit: u32,
    item: impl Fn(&PgRow) -> Result<T, sqlx::Error>,
) -> Result<ScanPage<T>, sqlx::Error> {
    let items = rows.iter().map(item).collect::<Result<Vec<_>, _>>()?;
    Ok(ScanPage { items, next: next_cursor(rows, limit)? })
}

/// 下一页游标：读满一页（含被跳过的行）才取本页最后一行，读不满说明游标之后已无待办。
pub(crate) fn next_cursor(rows: &[PgRow], limit: u32) -> Result<Option<ScanCursor>, sqlx::Error> {
    let last = rows.last().map(row_cursor).transpose()?;
    Ok(full_page_cursor(rows.len(), limit, last))
}

/// 已把行映射成自带游标的结构时使用：行数读满一页才给出最后一行的游标。
pub(crate) fn full_page_cursor(
    row_count: usize,
    limit: u32,
    last: Option<ScanCursor>,
) -> Option<ScanCursor> {
    if usize::try_from(limit).is_ok_and(|limit| row_count == limit) { last } else { None }
}
