//! 出价记录和履约进度的场次选择与地址查询提交。

use auction_web_shared::QueryWriteMode;
use dioxus::dioxus_core::needs_update;
use dioxus::prelude::*;

use crate::surface::{PublicBoardQuery, read_public_query, write_public_query};

/// 读取页从原生选择器应用的世界或场次。
#[derive(Clone)]
pub(super) enum ReadSelection {
    World(String),
    Auction(String),
}

impl ReadSelection {
    /// 以最新地址为起点切换读取目标。
    pub(super) fn select(self, state: Signal<PublicBoardQuery>) {
        self.select_with(state, read_public_query(), write_public_query);
    }

    /// 用同一次选择更新目标、筛选和页码。
    pub(super) fn select_with(
        self,
        state: Signal<PublicBoardQuery>,
        mut next: PublicBoardQuery,
        write_query: impl FnOnce(&PublicBoardQuery, QueryWriteMode) -> bool,
    ) -> bool {
        match self {
            Self::World(world) => {
                // 主世界使用缺省 URL，非主世界才写入 world。
                next.world = (world != "main").then_some(world);
                next.auction_id = None;
            }
            Self::Auction(auction_id) => next.auction_id = Some(auction_id),
        }
        next.bid_log_lot = None;
        next.page = 1;
        let written = commit_read_query_with(state, next, QueryWriteMode::Push, write_query);
        // 原生选择器已跟随用户选择，拒写时重绘已应用选项，供用户再次选择。
        if !written {
            needs_update();
        }
        written
    }
}

/// 从地址读出最近的已应用查询，再应用自动纠偏。
pub(super) fn update_read_query(
    state: Signal<PublicBoardQuery>,
    update: impl FnOnce(&mut PublicBoardQuery),
    mode: QueryWriteMode,
) {
    let mut next = read_public_query();
    update(&mut next);
    commit_read_query(state, next, mode);
}

/// 提交从当前已应用状态派生的查询。
pub(super) fn commit_read_query(
    state: Signal<PublicBoardQuery>,
    next: PublicBoardQuery,
    mode: QueryWriteMode,
) {
    commit_read_query_with(state, next, mode, write_public_query);
}

/// 以浏览器确认的地址为已应用查询，供后续读取和草稿同步消费。
pub(super) fn commit_read_query_with(
    mut state: Signal<PublicBoardQuery>,
    next: PublicBoardQuery,
    mode: QueryWriteMode,
    write_query: impl FnOnce(&PublicBoardQuery, QueryWriteMode) -> bool,
) -> bool {
    // 地址写入成功才推进查询信号，拒写时列表与草稿仍属于原查询。
    if !write_query(&next, mode) {
        return false;
    }
    state.set(next);
    true
}
