//! 出价记录和履约进度的场次选择与地址查询提交。

use auction_web_shared::{QueryWriteMode, t};
use dioxus::dioxus_core::needs_update;
use dioxus::prelude::*;

use super::directory::RoundDirectory;
use crate::surface::{
    AuctionSelection, FilterParameter, PublicBoardQuery, QueryCommit, commit_auction_selection,
    page_pathname, public_page_within_offset, read_page_query,
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
            if read_page_query().world.invalid() {
                return std::future::pending().await;
            }
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
    Clear,
}

impl ReadSelection {
    /// 以最新地址为起点切换读取目标。
    pub(super) fn select(self, state: Signal<PublicBoardQuery>, pathname: &str) -> bool {
        if page_pathname() != pathname {
            return false;
        }
        if matches!(&self, Self::World(world) if !matches!(world.as_str(), "main" | "isekai")) {
            return false;
        }
        let (world, auction) = self.into_fields();
        let mut keys = vec!["auction", "lot", "page"];
        if pathname == "/bidlog" {
            keys.push("bidlog_page");
        }
        if world.is_some() {
            keys.push("world");
        }
        let result = commit_auction_selection(
            state,
            AuctionSelection::Explicit(auction),
            Some(&keys),
            |next| Self::reset_filters(next, world),
        );
        let accepted = matches!(result, QueryCommit::Applied(_) | QueryCommit::Unchanged(_));
        // 原生选择器已跟随用户选择，拒写后恢复已应用选项。
        if !accepted {
            needs_update();
        }
        accepted
    }

    fn into_fields(self) -> (Option<Option<String>>, Option<String>) {
        match self {
            // 主世界使用缺省 URL，非主世界才写入 world。
            Self::World(world) => (Some((world != "main").then_some(world)), None),
            Self::Auction(auction_id) => (None, Some(auction_id)),
            Self::Clear => (None, None),
        }
    }

    fn reset_filters(next: &mut PublicBoardQuery, world: Option<Option<String>>) {
        if let Some(world) = world {
            next.world = world.into();
        }
        next.bid_log_lot = None;
        next.bid_log_page_alias = false;
        next.page = 1;
    }

    /// 供拍品筛选的计时器测试模拟选择并提交到地址替身。
    #[cfg(test)]
    pub(super) fn select_with(
        self,
        state: Signal<PublicBoardQuery>,
        mut next: PublicBoardQuery,
        write_query: impl FnOnce(&PublicBoardQuery, QueryWriteMode) -> bool,
    ) -> bool {
        let (world, auction) = self.into_fields();
        Self::reset_filters(&mut next, world);
        let next = next.with_auction_values(auction);
        let written = commit_read_query_with(state, next, QueryWriteMode::Push, write_query);
        // 原生选择器已跟随用户选择，拒写时重绘已应用选项，供用户再次选择。
        if !written {
            needs_update();
        }
        written
    }
}

/// 拍品筛选写入成功才同步草稿依赖的查询。该入口只写 lot 与页码。
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
    #[cfg(not(test))]
    let applied = read_page_query();
    #[cfg(test)]
    let applied = {
        let scope = dioxus::dioxus_core::Runtime::try_current()
            .and_then(|runtime| runtime.try_current_scope_id());
        // 注入写入函数的计时器测试没有页面地址，返回的草稿就是替身已提交的查询。
        if scope.is_some() && try_consume_context::<crate::surface::PageBrowserFixture>().is_some()
        {
            read_page_query()
        } else {
            next
        }
    };
    state.set(applied);
    true
}

/// 读取页的世界地址错误，与原生选择器关联并显示原始值。
#[component]
pub(super) fn InvalidReadWorld(world: FilterParameter) -> Element {
    let values = world.query_values();
    let message =
        if values.len() > 1 { t!("filter-error-duplicate") } else { t!("world-invalid-link") };
    let raw = values.join(" / ");
    rsx! {
        p { id: "public-read-world-error", class: "public-auction-empty__title", role: "alert",
            "{message}"
            bdi { " {raw}" }
        }
    }
}

/// 非法链接由玩家主动清除，目录才能选择可读场次。
#[component]
pub(super) fn InvalidReadAuction(on_recover: EventHandler<()>) -> Element {
    rsx! {
        div { class: "public-auction-empty",
            p { class: "public-auction-empty__title", role: "alert", {t!("auction-invalid-link")} }
            button {
                id: "public-read-auction-recovery",
                class: "public-auction-empty__retry",
                r#type: "button",
                onclick: move |_| on_recover.call(()),
                {t!("auction-view-current")}
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::dioxus_core::{ElementId, Mutation};
    use dioxus::prelude::*;

    use crate::api::{AuctionListPage, HistoricalAuction};
    use crate::surface::{
        PageBrowserFixture, PageReadFixture, PublicContentLoading, rendered_event_target,
        settle_page,
    };

    pub(in crate::surface::reads) const ROUND_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
    pub(in crate::surface::reads) const OLDER_ROUND_ID: &str =
        "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";

    pub(in crate::surface::reads) type MountedReadPage =
        (VirtualDom, Vec<Mutation>, Rc<RefCell<Vec<String>>>);

    pub(in crate::surface::reads) fn mount_read_page<T: 'static>(
        page: fn() -> Element,
        browser: &PageBrowserFixture,
        body: PageReadFixture<String, Result<T, String>>,
    ) -> (VirtualDom, Vec<Mutation>) {
        let applied = browser.clone();
        let directory = PageReadFixture::new(move |url: String| {
            assert!(applied.query().world.round_world().is_some(), "非法世界不读取场次目录");
            let auctions = if url.contains("status=settled") {
                [(ROUND_ID, 2), (OLDER_ROUND_ID, 1)]
                    .into_iter()
                    .map(|(id, sequence)| HistoricalAuction {
                        id: id.to_owned(),
                        world_id: "main".to_owned(),
                        sequence,
                        status: "settled".to_owned(),
                        scheduled_open_at: "2026-11-14T12:00:00Z".to_owned(),
                        opened_at: None,
                        final_phase_at: None,
                        effective_end_at: None,
                        settled_at: None,
                        lot_count: 0,
                        sold_count: 0,
                        unsold_count: 0,
                        sold_amount: 0,
                        state_version: 1,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Ok::<_, String>(AuctionListPage {
                total: Some(auctions.len() as i64),
                auctions,
                page: Some(1),
                page_size: 200,
                cursor: None,
                next_cursor: None,
                has_more: None,
            })
        });
        let mut dom = VirtualDom::new(page)
            .with_root_context(browser.clone())
            .with_root_context(directory)
            .with_root_context(body)
            .with_root_context(PageReadFixture::new(|()| std::future::ready(())));
        let mut edits = dom.rebuild_to_vec().edits;
        edits.extend(settle_page(&mut dom));
        (dom, edits)
    }

    pub(in crate::surface::reads) fn invalid_auction_recovery(
        dom: &VirtualDom,
        edits: &[Mutation],
    ) -> ElementId {
        for message in ["The auction in this link is invalid.", "View current auction"] {
            assert!(edits.iter().any(|edit| matches!(
                edit, Mutation::CreateTextNode { value, .. } if value == message
            )));
        }
        dom.in_scope(ScopeId::APP, || {
            assert!(consume_context::<PublicContentLoading>().0.peek().is_empty());
        });
        rendered_event_target(dom, "button", "id", "public-read-auction-recovery", "onclick")
    }
}
