//! Public 历史拍卖列表。

use std::future::Future;

use auction_web_shared::validation::restore_select_value;
use auction_web_shared::{
    AsyncState, AsyncStateKind, QueryWriteMode, Time, format_credit_amount, t,
};
use dioxus::prelude::*;

use crate::api::AuctionListPage;
use crate::api::{HistoricalAuctionWithForum, fetch_auction_history_page};
use crate::surface::loading::use_content_loading;
use crate::surface::shell::use_history_world_options;
use crate::surface::{
    PageKey, PublicPageClampError, PublicPager, auction_status_class, commit_public_query_pairs,
    historical_auction_status_label, historical_bid_log_href, historical_fulfillment_href,
    historical_lots_href, page_pathname, public_page_matches, public_page_within_offset,
    query_values, set_public_page_size, use_page_query, use_public_page_bounds,
    use_public_query_pairs, world_label,
};

#[component]
pub(crate) fn AuctionList() -> Element {
    // 世界/状态/页码与地址栏双向同步。
    let query_state_signal = use_page_query();
    let pairs = use_public_query_pairs(query_state_signal);
    let world_filter = use_memo(move || {
        ListFilter::read(&pairs.read(), "world", "all", &["all", "main", "isekai"])
    });
    let status_filter = use_memo(move || {
        ListFilter::read(&pairs.read(), "status", "settled", &["settled", "final", "live"])
    });
    let filters_valid =
        use_memo(move || world_filter.read().valid() && status_filter.read().valid());
    let bounds = use_public_page_bounds(query_state_signal, AUCTIONS_PATH, |query| {
        [(PageKey::Page, query.page_size)]
    });
    let page_number = use_memo(move || query_state_signal.read().page.max(1));
    let page_size = use_memo(move || query_state_signal.read().page_size);
    let mut refresh_generation = use_signal(|| 0_u64);
    let mut world_options = use_history_world_options();
    let selected_world = world_filter.read().value().to_owned();
    let selected_status = status_filter.read().value().to_owned();
    let world_invalid = !world_filter.read().valid();
    let status_invalid = !status_filter.read().valid();
    // 历史深链的世界即使已从当前目录移除，也必须在筛选器中保留真实值。
    if !world_filter.read().unknown()
        && selected_world != "all"
        && !world_options.contains(&selected_world)
    {
        world_options.push(selected_world.clone());
    }
    let auctions = use_auction_list_read(refresh_generation, move || {
        let world = world_filter.read().value().to_owned();
        let status = status_filter.read().value().to_owned();
        let page = page_number();
        let size = page_size();
        let readable = filters_valid() && public_page_within_offset(page, size);
        let requested = (world.clone(), status.clone(), page, size);
        let world = (world != "all").then_some(world);
        async move {
            if !readable {
                return None;
            }
            let result = fetch_auction_history_page(world, status, page, size).await;
            Some((requested, result))
        }
    });
    let auction_state = auctions.read_unchecked();
    // 查询变化时旧快照不能参与列表展示或页码纠偏，同查询后台刷新仍保留旧值。
    let current_result = auction_state
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|((world, status, page, size), _)| {
            filters_valid()
                && world == world_filter.read().value()
                && status == status_filter.read().value()
                && *page == page_number()
                && *size == page_size()
        })
        .map(|(_, result)| result);
    // 没有当前查询的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
    let clamp_failed = bounds.failed(PageKey::Page);
    use_content_loading(match current_result {
        None => filters_valid() && !clamp_failed,
        Some(Ok(page)) => {
            !public_page_matches(page.page, page.page_size, page_number(), page_size())
        }
        Some(Err(_)) => false,
    });
    let content = match current_result {
        _ if clamp_failed => rsx! { PublicPageClampError { bounds } },
        None => rsx! {},
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page))
            if !public_page_matches(page.page, page.page_size, page_number(), page_size()) =>
        {
            rsx! {}
        }
        Some(Ok(page)) => rsx! {
            AuctionHistoryPage {
                page: page.clone(),
                page_number: page_number(),
                on_page_size_change: move |size| page_pathname() == AUCTIONS_PATH
                    && set_public_page_size(query_state_signal, size),
                on_page_change: move |page| PageKey::Page.write(query_state_signal, AUCTIONS_PATH, page, QueryWriteMode::Push),
                on_page_clamp: move |page| PageKey::Page.write(query_state_signal, AUCTIONS_PATH, page, QueryWriteMode::Replace),
            }
        },
    };
    rsx! {
        // 筛选器保持挂载，切换查询、失败或重试时不会丢失键盘焦点。
        section { class: "public-auction-rounds", aria_label: t!("page-auctions-title"),
            div { class: "public-auction-rounds__controls",
                label {
                    {t!("auction-history-world")}
                    select {
                        name: "world",
                        value: selected_world.clone(),
                        aria_invalid: world_invalid.to_string(),
                        aria_describedby: world_invalid.then_some("public-auction-rounds-world-error"),
                        onchange: move |event| {
                            let value = event.value();
                            let pairs = (value != "all").then(|| ("world".to_owned(), value))
                                .into_iter().collect::<Vec<_>>();
                            if page_pathname() != AUCTIONS_PATH || !commit_public_query_pairs(
                                query_state_signal, &["world", "auction", "type", "page"], &pairs, QueryWriteMode::Push,
                            ) {
                                restore_select_value(&event, &selected_world);
                            }
                        },
                        if world_filter.read().unknown() {
                            option { value: world_filter.read().value().to_owned(), disabled: true, selected: true,
                                {world_filter.read().value().to_owned()}
                            }
                        }
                        option { value: "all", {t!("auction-history-world-all")} }
                        for world in world_options {
                            option { value: world, {world_label(&world)} }
                        }
                    }
                }
                if world_invalid {
                    p { id: "public-auction-rounds-world-error", class: "public-history-field-error", role: "status",
                        {world_filter.read().message(t!("history-filter-error-world"))}
                    }
                }
                label {
                    {t!("auction-history-status")}
                    select {
                        name: "status",
                        value: selected_status.clone(),
                        aria_invalid: status_invalid.to_string(),
                        aria_describedby: status_invalid.then_some("public-auction-rounds-status-error"),
                        onchange: move |event| {
                            if page_pathname() != AUCTIONS_PATH || !commit_public_query_pairs(
                                query_state_signal, &["status", "page"], &[("status".to_owned(), event.value())], QueryWriteMode::Push,
                            ) {
                                restore_select_value(&event, &selected_status);
                            }
                        },
                        if status_filter.read().unknown() {
                            option { value: status_filter.read().value().to_owned(), disabled: true, selected: true,
                                {status_filter.read().value().to_owned()}
                            }
                        }
                        option { value: "settled", {t!("auction-history-status-settled")} }
                        option { value: "final", {t!("auction-history-status-final")} }
                        option { value: "live", {t!("auction-history-status-live")} }
                    }
                }
                if status_invalid {
                    p { id: "public-auction-rounds-status-error", class: "public-history-field-error", role: "status",
                        {status_filter.read().message(t!("auction-history-status-invalid"))}
                    }
                }
            }
            {content}
        }
    }
}

const AUCTIONS_PATH: &str = "/auctions";

#[derive(Clone, PartialEq)]
struct ListFilter {
    raw: Vec<String>,
    default: &'static str,
    unknown: bool,
}

impl ListFilter {
    fn read(
        pairs: &[(String, String)],
        key: &str,
        default: &'static str,
        allowed: &[&str],
    ) -> Self {
        let raw: Vec<_> = query_values(pairs, key).map(str::to_owned).collect();
        let unknown = raw
            .first()
            .is_some_and(|value| !value.is_empty() && !allowed.contains(&value.as_str()));
        Self { raw, default, unknown }
    }

    fn value(&self) -> &str {
        self.raw.first().filter(|value| !value.is_empty()).map_or(self.default, String::as_str)
    }

    fn valid(&self) -> bool {
        !self.unknown && self.raw.len() <= 1
    }

    fn unknown(&self) -> bool {
        self.unknown
    }

    fn message(&self, invalid: String) -> String {
        if self.raw.len() > 1 {
            let values =
                self.raw.iter().map(|value| format!("{value:?}")).collect::<Vec<_>>().join(" / ");
            format!("{} {values}", t!("filter-error-duplicate"))
        } else if self.unknown {
            format!("{invalid} {:?}", self.value())
        } else {
            invalid
        }
    }
}

fn use_auction_list_read<F: Future + 'static>(
    retry_generation: Signal<u64>,
    mut read: impl FnMut() -> F + 'static,
) -> Resource<F::Output> {
    let shell_generation = use_context::<Signal<u64>>();
    use_resource(move || {
        // 外壳使目录缓存失效后，同一查询也须重读。页内重试独立推进本页代号。
        let _ = shell_generation();
        let _ = retry_generation();
        read()
    })
}

#[component]
pub(crate) fn AuctionHistoryPage(
    page: AuctionListPage<HistoricalAuctionWithForum>,
    page_number: i64,
    on_page_change: Callback<i64, bool>,
    on_page_clamp: Callback<i64, bool>,
    on_page_size_change: Callback<i64, bool>,
) -> Element {
    let page_size = page.page_size.max(1);
    rsx! {
        div {
            if page.auctions.is_empty() {
                div { class: "public-auction-empty", role: "status",
                    p { class: "public-auction-empty__title", {t!("auction-history-empty-title")} }
                    p { class: "public-auction-empty__hint", {t!("auction-history-empty-hint")} }
                }
            } else {
                AuctionHistoryTable { rows: page.auctions }
            }
            PublicPager {
                page_level: true,
                page: page_number,
                page_size,
                total: page.total,
                has_more: page.has_more.unwrap_or(false),
                label: t!("page-auctions-title"),
                on_page_change,
                on_page_clamp,
                on_page_size_commit: move |size| Some(on_page_size_change.call(size)),
                report_clamp_failure: true,
            }
        }
    }
}

#[component]
pub(crate) fn AuctionHistoryTable(rows: Vec<HistoricalAuctionWithForum>) -> Element {
    rsx! {
        div { class: "public-auction-rounds__table",
            table {
                aria_label: t!("page-auctions-title"),
                // “查看”一列由 public.css 改成刚好容纳四个入口的固定宽度，最后一项只是没有该样式时的
                // 回退；其余列按各自的百分比分配剩余宽度。
                auction_web_shared::TableColumns { layout: "public-auction-history", widths: vec![12, 7, 24, 6, 6, 6, 18, 21] }
                thead {
                    tr {
                        th { scope: "col", {t!("auction-history-number")} }
                        th { scope: "col", {t!("auction-history-world")} }
                        th { scope: "col", {t!("auction-history-ended")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-lots")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-sold")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-unsold")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-total")} }
                        th { scope: "col", {t!("auction-history-links")} }
                    }
                }
                tbody {
                    for auction in rows {
                        {
                            let forum_thread_url = auction.forum_thread_url;
                            let auction = auction.auction;
                            let ended = auction.settled_at.clone()
                                .or_else(|| auction.effective_end_at.clone())
                                .or_else(|| auction.opened_at.clone())
                                .unwrap_or_else(|| auction.scheduled_open_at.clone());
                            let lots_href = historical_lots_href(&auction.id, &auction.world_id);
                            let bid_log_href = historical_bid_log_href(&auction.id, &auction.world_id);
                            // 只有已结算场次有履约进度；进行中的场次不给入口。
                            let fulfillment_href = (auction.status == "settled")
                                .then(|| historical_fulfillment_href(&auction.id, &auction.world_id));
                            let status_class = auction_status_class(&auction.status);
                            let status_label = historical_auction_status_label(&auction.status);
                            rsx! {
                                tr {
                                    td {
                                        class: "public-auction-rounds__round",
                                        "data-label": t!("auction-history-number"),
                                        "#{auction.sequence}"
                                        span {
                                            class: "public-auction-rounds__status public-auction-rounds__status--{status_class}",
                                            {status_label}
                                        }
                                    }
                                    td { "data-label": t!("auction-history-world"), "{world_label(&auction.world_id)}" }
                                    td { "data-label": t!("auction-history-ended"), Time { value: ended } }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-lots"), "{auction.lot_count}" }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-sold"), "{auction.sold_count}" }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-unsold"), "{auction.unsold_count}" }
                                    td { class: "public-auction-rounds__total", "data-label": t!("auction-history-total"), "{format_credit_amount(auction.sold_amount)}" }
                                    td { class: "public-auction-rounds__links-cell", "data-label": t!("auction-history-links"),
                                        div { class: "public-auction-rounds__links",
                                            Link { to: lots_href, {t!("auction-history-link-lots")} }
                                            Link { to: bid_log_href, {t!("auction-history-link-bidlog")} }
                                            if let Some(fulfillment_href) = fulfillment_href {
                                                Link { to: fulfillment_href, {t!("auction-history-link-fulfillment")} }
                                            }
                                            if let Some(forum_thread_url) = forum_thread_url {
                                                a {
                                                    href: forum_thread_url,
                                                    target: "_blank",
                                                    rel: "noopener noreferrer",
                                                    {t!("auction-history-link-forum")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::ready;
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::{AuctionList, ListFilter, use_auction_list_read};
    use crate::api::{AuctionListPage, HistoricalAuctionWithForum, WorldStatus, Worlds};
    use crate::surface::reads::tests::select_values;
    use crate::surface::shell::WorldCatalog;
    use crate::surface::{
        PageBrowserFixture, PageReadFixture, assert_selected_error, dispatch_page_event,
        page_event_target, page_test_contexts, run_page_events, settle_page,
    };

    #[test]
    fn auction_list_status_preserves_invalid_and_duplicate_values_until_corrected() {
        let read =
            |pairs| ListFilter::read(pairs, "status", "settled", &["settled", "final", "live"]);
        assert!(read(&[]).valid());
        let raw = [("status".to_owned(), "unrecognized".to_owned())];
        assert!(!read(&raw).valid());
        assert_eq!(read(&raw).value(), "unrecognized");
        let duplicates =
            [("status".to_owned(), "live".to_owned()), ("status".to_owned(), "final".to_owned())];
        assert!(!read(&duplicates).valid());
        assert!(read(&duplicates[1..]).valid());
    }

    fn auction_list() -> Element {
        page_test_contexts();
        let catalog = use_context_provider(WorldCatalog::new);
        if let Some(control) = try_consume_context::<Rc<Cell<Option<WorldCatalog>>>>() {
            control.set(Some(catalog));
        }
        rsx! { AuctionList {} }
    }

    #[test]
    fn auction_list_uses_the_shared_catalog_without_reading_worlds() {
        let browser = PageBrowserFixture::new("/auctions", "status=unknown");
        let control = Rc::new(Cell::new(None::<WorldCatalog>));
        let requests = Rc::new(Cell::new(0));
        let mut dom = VirtualDom::new(auction_list)
            .with_root_context(browser)
            .with_root_context(control.clone())
            .with_root_context(PageReadFixture::new({
                let requests = requests.clone();
                move |()| {
                    requests.update(|count| count + 1);
                    Err::<Worlds, _>("unavailable".to_owned())
                }
            }));
        dom.rebuild_in_place();
        settle_page(&mut dom);
        assert_eq!(select_values(&dom, "name", "world"), ["all", "main", "isekai"]);
        dom.in_runtime(|| {
            control.get().unwrap().0.set(Some(Worlds {
                worlds: [("isekai".to_owned(), WorldStatus { enabled: false })].into(),
            }));
        });
        settle_page(&mut dom);
        assert_eq!(select_values(&dom, "name", "world"), ["all", "isekai"]);
        assert_eq!(requests.get(), 0);
    }

    #[test]
    fn auction_list_first_frame_selects_and_describes_both_unknown_values() {
        let browser =
            PageBrowserFixture::new("/auctions", "world=outside%20world&status=unknown%20phase");
        let mut dom = VirtualDom::new(auction_list).with_root_context(browser);
        let initial = dom.rebuild_to_vec();
        assert_selected_error(
            &dom,
            &initial.edits,
            "outside world",
            "public-auction-rounds-world-error",
            "Select a world or All worlds.",
        );
        assert_selected_error(
            &dom,
            &initial.edits,
            "unknown phase",
            "public-auction-rounds-status-error",
            "Select a valid auction status.",
        );
    }

    #[test]
    fn auction_list_select_changes_commit_world_and_status_to_the_live_signal() {
        run_page_events(
            module_path!(),
            "auction_list_select_changes_commit_world_and_status_to_the_live_signal",
            || {
                let browser = PageBrowserFixture::new(
                    "/auctions",
                    "world=outside&status=unknown&page=4&keep=yes",
                );
                let auctions = PageReadFixture::new(|url: String| {
                    assert_eq!(url, "/api/v2/auctions?page=1&page_size=20&status=live&world=main");
                    Ok::<_, String>(AuctionListPage::<HistoricalAuctionWithForum> {
                        auctions: Vec::new(),
                        page: Some(1),
                        page_size: 20,
                        total: Some(0),
                        cursor: None,
                        next_cursor: None,
                        has_more: None,
                    })
                });
                let mut dom = VirtualDom::new(auction_list)
                    .with_root_context(browser.clone())
                    .with_root_context(auctions);
                let initial = dom.rebuild_to_vec();
                let world = page_event_target(&initial.edits, "change", "value", "outside");
                let status = page_event_target(&initial.edits, "change", "value", "unknown");
                dispatch_page_event(&dom, "change", world, "main");
                settle_page(&mut dom);
                assert_eq!(browser.state().world.as_deref(), Some("main"));
                assert_eq!(browser.value("status").as_deref(), Some("unknown"));
                dispatch_page_event(&dom, "change", status, "live");
                settle_page(&mut dom);
                assert_eq!(browser.state().status.as_deref(), Some("live"));
                assert_eq!(browser.state().page, 1);
                assert_eq!(browser.value("keep").as_deref(), Some("yes"));
                assert!(browser.writes().iter().all(|write| write.accepted));
            },
        );
    }

    #[derive(Default)]
    struct ReadControl {
        shell: Cell<Option<Signal<u64>>>,
        retry: Cell<Option<Signal<u64>>>,
        requests: RefCell<Vec<&'static str>>,
    }

    #[component]
    fn ReadHarness() -> Element {
        let control = consume_context::<Rc<ReadControl>>();
        let shell = use_context_provider(|| Signal::new(0_u64));
        let retry = use_signal(|| 0_u64);
        let status = use_signal(|| "live");
        control.shell.set(Some(shell));
        control.retry.set(Some(retry));
        let _list = use_auction_list_read(retry, move || {
            control.requests.borrow_mut().push(status());
            ready(())
        });
        rsx! {}
    }

    #[test]
    fn auction_list_rereads_the_same_query_for_shell_refresh_and_local_retry() {
        let control = Rc::new(ReadControl::default());
        let mut dom = VirtualDom::new(ReadHarness).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.process_events();
        assert_eq!(&*control.requests.borrow(), &["live"]);

        let mut retry = control.retry.get().expect("挂载重试信号");
        dom.in_runtime(|| retry.set(1));
        dom.process_events();
        assert_eq!(&*control.requests.borrow(), &["live", "live"]);

        let mut shell = control.shell.get().expect("挂载外壳信号");
        dom.in_runtime(|| shell.set(1));
        dom.process_events();
        assert_eq!(&*control.requests.borrow(), &["live", "live", "live"]);
    }
}
