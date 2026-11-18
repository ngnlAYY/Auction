//! 出价记录和履约进度的场次选择与地址查询提交。

use auction_web_shared::QueryWriteMode;
use dioxus::dioxus_core::needs_update;
use dioxus::prelude::*;

use super::directory::RoundDirectory;
use crate::surface::{
    PublicBoardQuery, page_pathname, public_page_within_offset, read_page_query, write_page_pairs,
};

type RoundListQuery = (String, String, Option<String>, i64, i64);
type RoundBodyResource<T> = Resource<(RoundListQuery, Result<T, String>)>;

/// 目录确认、筛选和地址页码共同决定正文请求。同查询重读时保留已完成快照。
pub(super) fn use_round_body<T: 'static>(
    directory: RoundDirectory,
    requested_lot: impl Fn() -> Option<Option<String>> + 'static,
    page: Memo<i64>,
    size: Memo<i64>,
    fetch: impl AsyncFn(String, String, Option<String>, i64, i64) -> Result<T, String> + Copy + 'static,
) -> (RoundBodyResource<T>, bool) {
    let request = use_memo(move || {
        let round = directory.readable()?;
        let lot = requested_lot()?;
        let page = page();
        let size = size();
        public_page_within_offset(page, size).then_some((round, lot, page, size))
    });
    let data = use_resource(move || {
        let requested = request();
        async move {
            let Some((round, lot, page, size)) = requested else {
                return std::future::pending().await;
            };
            let key = (round.id.clone(), round.world.clone(), lot.clone(), page, size);
            (key, fetch(round.id, round.world, lot, page, size).await)
        }
    });
    (data, request.read().is_some())
}

/// 读取页从原生选择器应用的世界或场次。
#[derive(Clone)]
pub(super) enum ReadSelection {
    World(String),
    Auction(String),
}

impl ReadSelection {
    /// 以最新地址为起点切换读取目标。
    pub(super) fn select(self, state: Signal<PublicBoardQuery>, pathname: &str) -> bool {
        if page_pathname() != pathname {
            return false;
        }
        let change_world = matches!(self, Self::World(_));
        self.select_with(state, read_page_query(), |next, mode| {
            let mut keys = vec!["auction", "lot", "page"];
            if pathname == "/bidlog" {
                keys.push("bidlog_page");
            }
            if change_world {
                keys.push("world");
            }
            let mut pairs = Vec::new();
            if change_world && let Some(ref world) = next.world {
                pairs.push(("world".to_owned(), world.clone()));
            }
            if let Some(ref id) = next.auction_id {
                pairs.push(("auction".to_owned(), id.clone()));
            }
            write_page_pairs(&keys, &pairs, mode)
        })
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
        next.bid_log_page_alias = false;
        next.page = 1;
        let written = commit_read_query_with(state, next, QueryWriteMode::Push, write_query);
        // 原生选择器已跟随用户选择，拒写时重绘已应用选项，供用户再次选择。
        if !written {
            needs_update();
        }
        written
    }
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
