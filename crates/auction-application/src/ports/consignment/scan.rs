//! Worker 待办扫描的键集分页：一次扫描内按 `(排序键, 主键)` 走完全部待办。
//!
//! 保护不变量 4、5：游标只活在本次扫描里，不跨轮、不持久，不作为任何事实来源；待办是否存在只由持久事实决定。
//! 某些待办因暂态未就绪、临时缺证或确定拒绝而不写任何事实，下一轮仍会被选中；若每次都只取排序最前的一页，
//! 这些项会占满整页、饿死排在后面的就绪项。键集分页让一次扫描走完全部待办，被跳过的项同样推进游标。

use auction_time::Timestamp;
use uuid::Uuid;

/// 一次扫描内的键集游标：上一页最后一行待办的排序时刻与主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanCursor {
    /// 该待办来源的排序时刻列。
    pub sort_at: Timestamp,
    /// 该待办来源的主键，用来在同一时刻内排定先后。
    pub tie_id: Uuid,
}

/// 一页待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPage<T> {
    /// 本页待办（被来源跳过的行不在其中，但已计入游标）。
    pub items: Vec<T>,
    /// 读满一页时为本页最后一行的游标；读不满即本次扫描结束（`None`）。
    pub next: Option<ScanCursor>,
}

impl<T> ScanPage<T> {
    /// 没有后续页的一页，供只有一页的来源与测试替身使用。
    pub fn last(items: Vec<T>) -> Self {
        Self { items, next: None }
    }
}
