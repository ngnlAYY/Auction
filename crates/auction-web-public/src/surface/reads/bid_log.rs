//! 公开竞价日志页。

use auction_web_shared::TruncatedText;
use auction_web_shared::t;
use auction_web_shared::validation::restore_select_value;
use auction_web_shared::{QueryWriteMode, Time, format_credit_amount};
use dioxus::prelude::*;

use super::directory::{RoundOptionsError, RoundPage, use_round_directory};
use super::lot_filter::use_lot_filter_draft;
use super::query::{InvalidReadAuction, InvalidReadWorld, ReadSelection, use_round_body};
use crate::api::{BidLogEntry, BidLogPage, fetch_bid_log_page, fetch_worlds, lot_code_for};
use crate::surface::read_page_query;
use crate::surface::{
    BidHistoryParticipant, PublicPager, bid_history_rows, public_page_matches, supported_worlds,
    use_content_loading, use_page_query, world_label,
};
use crate::surface::{
    LotDetailLookupButton, LotDetailLookupHost, use_auction_bid_refresh, use_lot_detail_lookup,
};
use crate::surface::{
    PageKey, PublicPageClampError, page_pathname, public_page_within_offset, set_public_page_size,
    use_public_page_bounds,
};

/// lot 筛选格式提示的元素 ID，供输入框 `aria-describedby` 引用。
const LOT_FILTER_ERROR_ID: &str = "public-auction-bid-log-filter-error";

pub(crate) fn bid_log() -> Element {
    let pathname = RoundPage::BidLog.pathname();
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 世界/拍卖/页码/lot 筛选与地址栏双向同步。
    let query_state_signal = use_page_query();
    let world_filter =
        use_memo(move || query_state_signal.read().world.round_world().map(str::to_owned));
    let invalid_world = world_filter.read().is_none();
    let page_number = use_memo(move || query_state_signal.read().page);
    let page_size = use_memo(move || query_state_signal.read().page_size);
    // 缺省场次与第一页一同提交，无需先替换页码。
    let bounds = use_public_page_bounds(query_state_signal, pathname, |query| {
        query.world.round_world()?;
        query.auction_id().map(|_| (PageKey::BidLog, query.page_size))
    });
    let lot_draft = use_lot_filter_draft(query_state_signal, pathname);
    // 旧页码别名只在仍属于出价记录且可按既定数量读取时原地收敛。
    use_effect(move || {
        let (has_auction, alias, page, page_size) = {
            let query = query_state_signal.read();
            (
                query.auction_id().is_some() && !query.world.invalid(),
                query.bid_log_page_alias,
                query.page,
                query.page_size,
            )
        };
        // 成功替换会写入同一个 Signal，此时前面的读借用必须已释放。
        if has_auction
            && alias
            && public_page_within_offset(page, page_size)
            && page_pathname() == pathname
        {
            let current = read_page_query();
            if current.auction_id().is_some()
                && !current.world.invalid()
                && current.bid_log_page_alias
                && current.page == page
                && current.page_size == page_size
            {
                PageKey::BidLog.write(
                    query_state_signal,
                    pathname,
                    current.page,
                    QueryWriteMode::Replace,
                );
            }
        }
    });

    let worlds = use_resource(fetch_worlds);
    let selected_world = world_filter().unwrap_or_default();
    let mut world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    if !invalid_world && !world_options.contains(&selected_world) {
        world_options.push(selected_world.clone());
    }
    let directory = use_round_directory(
        RoundPage::BidLog,
        query_state_signal,
        world_filter,
        refresh_generation,
    );
    let (data, readable) = use_round_body(
        directory,
        move || lot_draft.requested_lot().ok(),
        page_number,
        page_size,
        async |auction_id, world, lot, page, size| {
            fetch_bid_log_page(auction_id, page, world, lot, size).await
        },
    );
    let bid_scope = readable.then(|| directory.readable()).flatten();
    use_auction_bid_refresh(data, bid_scope.as_ref().map(|round| round.id.as_str()));
    let lookup = use_lot_detail_lookup();
    let current_page = page_number();
    let current_lot_filter = lot_draft.applied_value();
    let draft_value = lot_draft.value();
    let filter_error = lot_draft.error(|| t!("bidlog-filter-invalid"));
    let selected_auction = directory.selected();
    let selected = selected_auction.as_ref().map_or_else(String::new, |auction| auction.id.clone());
    let auction_options = directory.options();
    let applied_lot = lot_draft.requested_lot();
    let data_snapshot = data.read_unchecked();
    let current_result = data_snapshot
        .as_ref()
        .filter(|((auction_id, world, lot, page, size), _)| {
            selected_auction
                .as_ref()
                .is_some_and(|auction| &auction.id == auction_id && &auction.world_id == world)
                && applied_lot.as_ref().is_ok_and(|applied| applied == lot)
                && *page == current_page
                && *size == page_size()
        })
        .map(|(_, result)| result);
    let mut loading = false;
    let body = match current_result {
        _ if invalid_world => {
            rsx! { InvalidReadWorld { world: query_state_signal.read().world.clone() } }
        }
        _ if directory.invalid_auction() => rsx! {
            InvalidReadAuction {
                on_recover: move |_| { ReadSelection::Clear.select(query_state_signal, pathname); },
            }
        },
        _ if bounds.failed(PageKey::BidLog) => rsx! { PublicPageClampError { bounds } },
        _ if directory.failed() || matches!(current_result, Some(Err(_error))) => rsx! {
            div { class: "public-auction-empty", role: "alert",
                p { class: "public-auction-empty__title", {t!("data-error")} }
                button {
                    class: "public-auction-empty__retry",
                    r#type: "button",
                    onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                    {t!("data-retry")}
                }
            }
        },
        _ if directory.empty() => rsx! {
            div { class: "public-auction-empty", role: "status",
                p { class: "public-auction-empty__title", {t!("data-empty")} }
            }
        },
        // 非法筛选与拒写后的未选场次均可在控件处修正，不进入加载态。
        _ if applied_lot.is_err() || directory.correction_failed() => rsx! {},
        Some(Ok(page))
            if public_page_matches(page.page, page.page_size, current_page, page_size()) =>
        {
            rsx! {
                BidLogTable {
                    page: page.clone(),
                    current_page,
                    world: selected_world.clone(),
                    lot_filter: current_lot_filter.clone(),
                    on_page_change: move |page| PageKey::BidLog.write(query_state_signal, pathname, page, QueryWriteMode::Push),
                    on_page_clamp: move |page| PageKey::BidLog.write(query_state_signal, pathname, page, QueryWriteMode::Replace),
                    on_page_size_change: move |size| page_pathname() == pathname && set_public_page_size(query_state_signal, size),
                }
            }
        }
        _ => {
            loading = true;
            rsx! {}
        }
    };
    use_content_loading(loading);

    rsx! {
        section { class: "public-auction-bid-log-page", aria_label: t!("page-bidlog-title"),
            div { class: "public-auction-bid-log-controls",
                label { r#for: "public-auction-bid-log-world",
                    span { {t!("world-label")} }
                    select {
                        id: "public-auction-bid-log-world",
                        name: "world",
                        autocomplete: "off",
                        value: selected_world.clone(),
                        aria_invalid: invalid_world.to_string(),
                        aria_describedby: invalid_world.then_some("public-read-world-error"),
                        disabled: world_options.is_empty(),
                        onchange: move |event| {
                            if !ReadSelection::World(event.value()).select(query_state_signal, pathname) {
                                restore_select_value(&event, &selected_world);
                            }
                        },
                        if invalid_world {
                            option { value: "", disabled: true, selected: true, {t!("world-label")} }
                        }
                        for world in &world_options {
                            option { value: world.clone(), selected: selected_world == *world, {world_label(world)} }
                        }
                    }
                }
                label { r#for: "public-auction-bid-log-round",
                    span { {t!("bidlog-round")} }
                    select {
                        id: "public-auction-bid-log-round",
                        name: "auction",
                        autocomplete: "off",
                        value: selected.clone(),
                        disabled: auction_options.is_empty(),
                        onfocus: move |_| directory.load_options(),
                        onchange: move |event| {
                            let id = event.value();
                            if !directory.selectable(&id) || !ReadSelection::Auction(id).select(query_state_signal, pathname) {
                                let applied = directory.selected().map_or_else(String::new, |round| round.id);
                                restore_select_value(&event, &applied);
                            }
                        },
                        option { value: "", disabled: true, selected: selected.is_empty(),
                            {t!("round-select-placeholder")}
                        }
                        for auction in auction_options.iter() {
                            option {
                                value: auction.id.clone(), selected: auction.id == selected,
                                {t!("bidlog-round-option", number: auction.sequence,
                                    status: bid_log_status_label(&auction.status))}
                            }
                        }
                    }
                }
            }
            if directory.options_failed() && selected_auction.is_some() {
                RoundOptionsError { on_retry: move |_| refresh_generation.set(refresh_generation().saturating_add(1)) }
            }
            div { class: "public-auction-bid-log-heading",
                if let Some(ref auction) = selected_auction {
                    div { class: "public-auction-bid-log-round-header",
                        strong { class: "public-auction-bid-log-round-number",
                            {t!("bidlog-round-number", number: auction.sequence)}
                        }
                        span { class: "public-auction-bid-log-round-world",
                            {t!("bidlog-world", world: world_label(&auction.world_id))}
                        }
                        span { class: bid_log_status_class(&auction.status),
                            {bid_log_status_label(&auction.status)}
                        }
                    }
                }
                div { class: "public-auction-bid-log-toolbar",
                    label { r#for: "public-auction-bid-log-filter", {t!("bidlog-filter-label")} }
                    input {
                        id: "public-auction-bid-log-filter",
                        name: "lot",
                        autocomplete: "off",
                        r#type: "search",
                        enterkeyhint: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
                        value: draft_value,
                        placeholder: t!("bidlog-filter-placeholder"),
                        aria_invalid: filter_error.is_some().to_string(),
                        aria_describedby: filter_error.as_ref().map(|_| LOT_FILTER_ERROR_ID),
                        oninput: move |event| lot_draft.edit(event.value()),
                        onblur: move |_| lot_draft.blur(),
                    }
                    if !draft_value.is_empty() || !current_lot_filter.is_empty() || applied_lot.is_err() {
                        button {
                            r#type: "button",
                            onclick: move |_| lot_draft.clear(),
                            {t!("bidlog-filter-clear")}
                        }
                    }
                    if let Some(ref error) = filter_error {
                        p { id: LOT_FILTER_ERROR_ID, class: "public-auction-bid-log-toolbar__error", "{error}" }
                    }
                    span { class: "ui-pager__sr-only", aria_live: "polite", aria_atomic: "true",
                        if let Some(ref error) = filter_error { "{error}" }
                    }
                }
            }
            {body}
            // 抽屉宿主在记录读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。
            LotDetailLookupHost { lookup }
        }
    }
}

#[component]
pub(crate) fn BidLogTable(
    page: BidLogPage,
    current_page: i64,
    world: String,
    lot_filter: String,
    on_page_change: Callback<i64, bool>,
    on_page_clamp: Callback<i64, bool>,
    on_page_size_change: Callback<i64, bool>,
) -> Element {
    let has_more = page.has_more.unwrap_or(false);
    let bids = bid_history_rows(page.bids);
    rsx! {
        div {
            if bids.is_empty() {
                p { class: "public-auction-bid-log-empty",
                    if lot_filter.is_empty() {
                        {t!("bidlog-empty")}
                    } else {
                        {t!("bidlog-filter-empty")}
                    }
                }
            } else {
            div { class: "public-auction-bid-log-table-wrap",
                table {
                    class: "public-auction-bid-log-table",
                    aria_label: t!("page-bidlog-title"),
                    auction_web_shared::TableColumns {
                        layout: "public-bid-log",
                        widths: vec![20, 9, 31, 22, 12, 6],
                    }
                    thead {
                        tr {
                            th { scope: "col", {t!("bidlog-time")} }
                            th { scope: "col", {t!("bidlog-lot")} }
                            th { scope: "col", {t!("catalog-item")} }
                            th { scope: "col", {t!("bidlog-bidder")} }
                            th { scope: "col", class: "public-auction-bid-log-number", {t!("bidlog-amount")} }
                            th { scope: "col", class: "public-lot-view-cell", {t!("catalog-actions")} }
                        }
                    }
                    tbody {
                        for (order, bid) in bids.into_iter().enumerate() {
                            // 以服务端 bid id 作为稳定 key，避免翻页/筛选刷新后行状态错位。
                            BidLogRow { key: "{bid.id}", bid, world: world.clone(), order }
                        }
                    }
                }
            }
            }
            PublicPager {
                page: current_page,
                page_size: page.page_size,
                total: page.total,
                has_more,
                label: t!("page-bidlog-title"),
                on_page_change,
                on_page_clamp,
                on_page_size_commit: move |size| Some(on_page_size_change.call(size)),
                report_clamp_failure: true,
            }
        }
    }
}

#[component]
pub(crate) fn BidLogRow(bid: BidLogEntry, world: String, order: usize) -> Element {
    auction_web_shared::components::use_truncated_text_anchor_order(order);
    let lot_code = lot_code_for(&bid.equipment_category, bid.lot_number);
    let validity = if bid.valid { t!("bidlog-valid") } else { t!("bidlog-invalid") };
    rsx! {
        tr {
            class: if bid.valid { "public-auction-bid-log-leading" } else { "" },
            "aria-description": validity,
            td { "data-label": t!("bidlog-time"),
                Time { value: bid.created_at.clone() }
            }
            // 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。
            td { class: "public-auction-bid-log-code", "data-label": t!("bidlog-lot"), "{lot_code}" }
            // 名称外没有链接承接焦点，截断时由文本自身进入 Tab 顺序以查看完整名称。
            td { "data-label": t!("catalog-item"),
                TruncatedText { value: bid.equipment_name.clone(), size: "wide", portal: true }
            }
            td { class: "public-auction-bid-log-participant", "data-label": t!("bidlog-bidder"),
                BidHistoryParticipant {
                    name: bid.bidder_name,
                    source: bid.source,
                    source_post_number: bid.source_post_number,
                    source_post_url: bid.source_post_url,
                }
            }
            td {
                class: "public-auction-bid-log-amount public-auction-bid-log-number",
                "data-label": t!("bidlog-amount"),
                "{format_credit_amount(bid.amount)}"
            }
            td { class: "public-lot-view-cell", "data-label": t!("catalog-actions"),
                LotDetailLookupButton {
                    auction_id: bid.auction_id.clone(),
                    lot_id: bid.lot_id.clone(),
                    lot_code,
                    world: Some(world),
                }
            }
        }
    }
}

pub(crate) fn bid_log_status_label(status: &str) -> String {
    match status {
        "live" => t!("auction-history-status-live"),
        "final" => t!("auction-history-status-final"),
        "settled" => t!("auction-history-status-settled"),
        _ => t!("bidlog-status-unknown"),
    }
}

pub(crate) fn bid_log_status_class(status: &str) -> &'static str {
    match status {
        "live" => "public-auction-bid-log-round-status public-auction-bid-log-round-status-live",
        "final" => "public-auction-bid-log-round-status public-auction-bid-log-round-status-final",
        _ => "public-auction-bid-log-round-status",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{Future, poll_fn, ready};
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    #[cfg(not(target_arch = "wasm32"))]
    use auction_web_shared::components::TooltipBrowserFixture;
    use auction_web_shared::{QueryWriteMode, use_second_ticker};
    #[cfg(not(target_arch = "wasm32"))]
    use dioxus::core::{AttributeValue, Mutation};
    use dioxus::prelude::*;

    use super::super::query::tests::{
        MountedReadPage, OLDER_ROUND_ID, ROUND_ID, invalid_auction_recovery, mount_read_page,
    };
    use super::bid_log;
    #[cfg(not(target_arch = "wasm32"))]
    use super::{BidLogTable, use_lot_detail_lookup};
    use crate::api::{
        AuctionListPage, BidLogEntry, BidLogPage, HistoricalAuction, HttpResponseFixture,
        WorldStatus, Worlds,
    };
    use crate::surface::{
        LotDetailHttpFixture, PageBrowserFixture, PageReadFixture, PublicBidRefresh, PublicPager,
        dispatch_page_event, page_test_contexts, rendered_event_target, run_page_events,
        set_public_page_size, settle_page, use_page_query,
    };

    const LOT_ID: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000019";

    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Default)]
    struct BidTableFixture(Cell<Option<Signal<BidLogPage>>>);

    #[cfg(not(target_arch = "wasm32"))]
    fn bid_table_fixture() -> Element {
        page_test_contexts();
        use_lot_detail_lookup();
        let page = use_signal(|| drawer_bids("world=main"));
        use_context::<Rc<BidTableFixture>>().0.set(Some(page));
        rsx! {
            button { id: "outside", "Outside" }
            BidLogTable {
                page: page(), current_page: 1, world: "main", lot_filter: "",
                on_page_change: |_| true, on_page_clamp: |_| true, on_page_size_change: |_| true,
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bid_table_returns_focus_when_a_portal_is_closed_by_row_or_content_changes() {
        run_page_events(
            module_path!(),
            "bid_table_returns_focus_when_a_portal_is_closed_by_row_or_content_changes",
            || {
                for restore_focus in [true, false] {
                    let browser = Rc::new(TooltipBrowserFixture::new());
                    let fixture = Rc::new(BidTableFixture::default());
                    let mut dom = VirtualDom::new(bid_table_fixture)
                        .with_root_context(browser.clone())
                        .with_root_context(fixture.clone());
                    let initial = dom.rebuild_to_vec().edits;
                    let anchor = rendered_event_target(
                        &dom,
                        "span",
                        "data-full-value",
                        "Shield",
                        "onmouseenter",
                    );
                    let trigger = initial
                        .iter()
                        .find_map(|edit| match edit {
                            Mutation::SetAttribute {
                                id,
                                name: "id",
                                value: AttributeValue::Text(value),
                                ..
                            } if *id == anchor => Some(value.as_str()),
                            _ => None,
                        })
                        .expect("出价装备名触发器已挂载");
                    let tooltip = trigger.strip_suffix("-trigger").unwrap();
                    browser.add_focusable(trigger);
                    browser.add_focusable("outside");
                    settle_page(&mut dom);
                    dispatch_page_event(&dom, "mouseenter", anchor, "");
                    settle_page(&mut dom);
                    assert_eq!(browser.portal_text(tooltip).as_deref(), Some("Shield"));
                    let focus = if restore_focus { tooltip } else { "outside" };
                    let returned = if restore_focus { trigger } else { focus };
                    browser.focus_by_id(focus);
                    assert_eq!(browser.focus_id(), focus);

                    let mut page = fixture.0.get().unwrap();
                    let mut later = page.peek().bids[0].clone();
                    later.id.push_str("-later");
                    later.equipment_name = "Later Shield".to_owned();
                    dom.in_runtime(|| page.write().bids.push(later));
                    settle_page(&mut dom);
                    assert_eq!(browser.portal_text(tooltip).as_deref(), Some("Shield"));
                    assert_eq!(browser.focus_id(), focus);

                    let mut newest = page.peek().bids[0].clone();
                    newest.id.push_str("-newest");
                    newest.equipment_name = "New Shield".to_owned();
                    dom.in_runtime(|| page.write().bids.insert(0, newest));
                    settle_page(&mut dom);
                    assert_eq!(browser.portal_text(tooltip), None, "行移位时移除旧 portal");
                    assert_eq!(browser.focus_id(), returned);
                    dispatch_page_event(&dom, "click", anchor, "");
                    settle_page(&mut dom);
                    assert_eq!(browser.portal_text(tooltip).as_deref(), Some("Shield"));

                    browser.focus_by_id(focus);
                    dom.in_runtime(|| {
                        page.write().bids[1].equipment_name = "Updated Shield".to_owned();
                    });
                    settle_page(&mut dom);
                    assert_eq!(browser.portal_text(tooltip), None, "文字变化时移除旧 portal");
                    assert_eq!(browser.focus_id(), returned);
                    dispatch_page_event(&dom, "click", anchor, "");
                    settle_page(&mut dom);
                    assert_eq!(browser.portal_text(tooltip).as_deref(), Some("Updated Shield"));
                }
            },
        );
    }

    thread_local! {
        static DETAIL_READS: Cell<usize> = const { Cell::new(0) };
    }

    fn bid_log_page() -> Element {
        page_test_contexts();
        bid_log()
    }

    fn mount_bid_log(browser: &PageBrowserFixture, expected_page: i64) -> MountedReadPage {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let request_log = requests.clone();
        let applied = browser.clone();
        let body = PageReadFixture::new(move |url: String| {
            assert_eq!(applied.value("bidlog_page"), None);
            assert_eq!(
                applied.value("page"),
                (expected_page > 1).then(|| expected_page.to_string())
            );
            assert_eq!(applied.value("auction").as_deref(), Some(ROUND_ID));
            request_log.borrow_mut().push(url);
            Ok::<_, String>(BidLogPage {
                bids: Vec::new(),
                page: Some(expected_page),
                page_size: 20,
                total: Some(40),
                cursor: None,
                next_cursor: None,
                has_more: None,
            })
        });
        let (dom, edits) = mount_read_page(bid_log_page, browser, body);
        (dom, edits, requests)
    }

    #[test]
    fn invalid_bid_log_world_blocks_reads_until_an_explicit_selection() {
        run_page_events(
            module_path!(),
            "invalid_bid_log_world_blocks_reads_until_an_explicit_selection",
            || {
                for world in ["world=unknown", "world=main&world=isekai"] {
                    let browser = PageBrowserFixture::new("/bidlog", &format!("{world}&keep=yes"));
                    let (mut dom, edits, requests) = mount_bid_log(&browser, 1);
                    assert!(requests.borrow().is_empty());
                    assert!(browser.writes().is_empty());
                    assert!(edits.iter().any(|edit| matches!(
                    edit, dioxus::dioxus_core::Mutation::SetAttribute {
                        name: "aria-describedby", value: dioxus::dioxus_core::AttributeValue::Text(value), ..
                    } if value == "public-read-world-error"
                )));
                    let select = rendered_event_target(
                        &dom,
                        "select",
                        "id",
                        "public-auction-bid-log-world",
                        "onchange",
                    );
                    browser.allow_writes(false);
                    dispatch_page_event(&dom, "change", select, "main");
                    settle_page(&mut dom);
                    assert!(requests.borrow().is_empty());
                    assert!(browser.state().world.invalid());
                    browser.allow_writes(true);
                    dispatch_page_event(&dom, "change", select, "main");
                    settle_page(&mut dom);
                    assert_eq!(requests.borrow().len(), 1);
                    assert!(browser.value("world").is_none());
                    assert_eq!(browser.value("keep").as_deref(), Some("yes"));
                    assert!(requests.borrow()[0].ends_with("world=main"));
                }
            },
        );
    }

    #[test]
    fn a_bid_for_another_round_leaves_the_settled_bid_log_alone() {
        let browser = PageBrowserFixture::new("/bidlog", &format!("auction={ROUND_ID}"));
        let (mut dom, _, requests) = mount_bid_log(&browser, 1);
        assert_eq!(requests.borrow().len(), 1);
        for (auction, expected) in [(OLDER_ROUND_ID, 1), (ROUND_ID, 2)] {
            dom.in_scope(ScopeId::APP, || {
                consume_context::<crate::surface::PublicBidRefresh>().notify(auction, 0);
            });
            settle_page(&mut dom);
            assert_eq!(requests.borrow().len(), expected);
        }
    }

    fn bid_log_for_drawer() -> Element {
        use_second_ticker();
        bid_log_page()
    }

    fn drawer_directory(url: String) -> Result<AuctionListPage, String> {
        let (auction_id, world) = if url.contains("world=isekai") {
            (OLDER_ROUND_ID, "isekai")
        } else {
            (ROUND_ID, "main")
        };
        let auctions = url.contains("status=settled").then(|| HistoricalAuction {
            id: auction_id.to_owned(),
            world_id: world.to_owned(),
            sequence: 1,
            status: "settled".to_owned(),
            scheduled_open_at: "2026-11-14T12:00:00Z".to_owned(),
            opened_at: None,
            final_phase_at: None,
            effective_end_at: None,
            settled_at: None,
            lot_count: 1,
            sold_count: 1,
            unsold_count: 0,
            sold_amount: 1000,
            state_version: 1,
        });
        Ok(AuctionListPage {
            total: Some(i64::from(auctions.is_some())),
            auctions: auctions.into_iter().collect(),
            page: Some(1),
            page_size: 200,
            cursor: None,
            next_cursor: None,
            has_more: None,
        })
    }

    fn drawer_bids(url: &str) -> BidLogPage {
        let for_log = url.contains("world=main");
        let page_size = if url.contains("page_size=4") {
            4
        } else if url.contains("page_size=10") {
            10
        } else {
            20
        };
        let bids = for_log.then(|| BidLogEntry {
            id: "018f4c8e-7b2a-4c3d-8e4f-000000000020".to_owned(),
            auction_id: ROUND_ID.to_owned(),
            lot_id: LOT_ID.to_owned(),
            lot_number: 1,
            amount: 1000,
            equipment_name: "Shield".to_owned(),
            equipment_category: "Shield".to_owned(),
            bidder_name: "Buyer".to_owned(),
            source: None,
            source_post_number: None,
            source_post_url: None,
            valid: true,
            accepted_state_version: 1,
            fact_index: 1,
            mechanism: "direct".to_owned(),
            created_at: "2026-11-15T12:00:00Z".to_owned(),
            defeated_proxy: None,
        });
        BidLogPage {
            bids: bids.into_iter().collect(),
            page: Some(1),
            page_size,
            total: Some(5),
            cursor: None,
            next_cursor: None,
            has_more: None,
        }
    }

    fn drawer_detail() -> HttpResponseFixture {
        HttpResponseFixture {
            status: 200,
            etag: None,
            body: Some(
                serde_json::json!({
                    "code": "ok", "message": "拍品详情",
                    "data": {
                        "auction_id": ROUND_ID, "auction_status": "settled",
                        "projection_kind": "live", "presentation_mode": "live",
                        "bidding_open": false, "opens_at": "2026-11-14T12:00:00Z",
                        "preview_revision": 0,
                        "lot": {
                            "id": LOT_ID, "auction_id": ROUND_ID, "lot_number": 1,
                            "status": "unsold", "start_price": 1000, "current_price": 1000,
                            "next_bid": 1000, "has_leader": false, "seller_name": "Seller",
                            "state_version": 1,
                            "equipment": { "name": "Shield", "category": "Shield",
                                "equipment_group": "rare", "game_equipment_url": "" }
                        }
                    }
                })
                .to_string(),
            ),
        }
    }

    #[test]
    fn a_preserved_drawer_rereads_its_round_even_after_a_bid_for_the_new_world() {
        run_page_events(
            module_path!(),
            "a_preserved_drawer_rereads_its_round_even_after_a_bid_for_the_new_world",
            || {
                DETAIL_READS.set(0);
                let browser = PageBrowserFixture::new("/bidlog", &format!("auction={ROUND_ID}"));
                let requests = Rc::new(RefCell::new(Vec::new()));
                let bid_reads = PageReadFixture::new({
                    let requests = requests.clone();
                    move |url: String| {
                        let result = drawer_bids(&url);
                        requests.borrow_mut().push(url);
                        Ok::<_, String>(result)
                    }
                });
                let released = Rc::new(Cell::new(false));
                let wakers = Rc::new(RefCell::new(Vec::<Waker>::new()));
                let timers = PageReadFixture::new({
                    let released = released.clone();
                    let wakers = wakers.clone();
                    move |()| -> Pin<Box<dyn Future<Output = ()>>> {
                        let released = released.clone();
                        let wakers = wakers.clone();
                        Box::pin(poll_fn(move |context| {
                            if released.get() {
                                Poll::Ready(())
                            } else {
                                wakers.borrow_mut().push(context.waker().clone());
                                Poll::Pending
                            }
                        }))
                    }
                });
                let mut dom = VirtualDom::new(bid_log_for_drawer)
                    .with_root_context(browser.clone())
                    .with_root_context(PageReadFixture::new(drawer_directory))
                    .with_root_context(PageReadFixture::new(|()| {
                        Ok::<_, String>(Worlds {
                            worlds: ["main", "isekai"]
                                .into_iter()
                                .map(|world| (world.to_owned(), WorldStatus { enabled: true }))
                                .collect(),
                        })
                    }))
                    .with_root_context(bid_reads)
                    .with_root_context(timers)
                    .with_root_context(LotDetailHttpFixture(|url, _etag| {
                        assert_eq!(url, format!("/api/v1/lots/{LOT_ID}"));
                        DETAIL_READS.update(|calls| calls + 1);
                        Some(Box::pin(ready(Ok(drawer_detail()))))
                    }));
                dom.rebuild_in_place();
                settle_page(&mut dom);
                let view =
                    rendered_event_target(&dom, "button", "class", "public-lot-view", "onclick");
                dispatch_page_event(&dom, "click", view, "");
                settle_page(&mut dom);
                let history = rendered_event_target(
                    &dom,
                    "button",
                    "class",
                    "public-catalog-detail__more",
                    "onclick",
                );
                dispatch_page_event(&dom, "click", history, "");
                settle_page(&mut dom);
                assert_eq!(DETAIL_READS.get(), 1);

                let notify = |dom: &VirtualDom, auction_id| {
                    dom.in_scope(ScopeId::APP, || {
                        consume_context::<PublicBidRefresh>().notify(auction_id, 0);
                    });
                };
                notify(&dom, ROUND_ID);
                settle_page(&mut dom);
                assert_eq!(wakers.borrow().len(), 4, "目录与抽屉的三个读取均已排定");
                let world = rendered_event_target(
                    &dom,
                    "select",
                    "id",
                    "public-auction-bid-log-world",
                    "onchange",
                );
                dispatch_page_event(&dom, "change", world, "isekai");
                settle_page(&mut dom);
                assert_eq!(browser.state().auction_id(), Some(OLDER_ROUND_ID));
                notify(&dom, OLDER_ROUND_ID);
                settle_page(&mut dom);
                assert_eq!(DETAIL_READS.get(), 1);
                assert_eq!(requests.borrow().len(), 4, "合并窗口仍在等待");

                // 实时入口先使缓存过期。只读抽屉仍属于 A，页面的新目录已经属于 B。
                crate::api::invalidate_all();
                released.set(true);
                for waker in wakers.take() {
                    waker.wake();
                }
                settle_page(&mut dom);
                assert_eq!(DETAIL_READS.get(), 2);
                let log = requests.borrow();
                let round_a = format!("/api/v1/auctions/{ROUND_ID}/bid-log");
                let round_b = format!("/api/v1/auctions/{OLDER_ROUND_ID}/bid-log");
                for (path, parameters, expected) in [
                    (&round_a, "page_size=20&include=defeated_proxy&world=main", 1),
                    (&round_a, "page_size=4&include=defeated_proxy", 2),
                    (&round_a, "page_size=10&include=defeated_proxy", 2),
                    (&round_b, "page_size=20&include=defeated_proxy&world=isekai", 2),
                ] {
                    assert_eq!(
                        log.iter()
                            .filter(|url| url.starts_with(path) && url.ends_with(parameters))
                            .count(),
                        expected
                    );
                }
                assert_eq!(log.len(), 7);
            },
        );
    }

    #[test]
    fn old_alias_replace_releases_query_before_committing_and_reading_body() {
        let browser = PageBrowserFixture::new(
            "/bidlog",
            &format!("auction={ROUND_ID}&bidlog_page=2&keep=yes"),
        );
        let (_dom, _edits, requests) = mount_bid_log(&browser, 2);
        assert!(!browser.state().bid_log_page_alias);
        let writes = browser.writes();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].accepted);
        assert_eq!(writes[0].mode, QueryWriteMode::Replace);
        assert_eq!(writes[0].keys, ["page", "bidlog_page"]);
        assert_eq!(browser.value("keep").as_deref(), Some("yes"));
        assert_eq!(
            &*requests.borrow(),
            &[format!(
                "/api/v1/auctions/{ROUND_ID}/bid-log?page=2&page_size=20&include=defeated_proxy&world=main"
            )]
        );
    }

    #[test]
    fn invalid_auction_does_not_load_bid_log_body() {
        run_page_events(module_path!(), "invalid_auction_does_not_load_bid_log_body", || {
            for auction in
                ["auction=bad".to_owned(), format!("auction={ROUND_ID}&auction={ROUND_ID}")]
            {
                let browser = PageBrowserFixture::new(
                    "/bidlog",
                    &format!("{auction}&bidlog_page=4&lot=Wrong%21&keep=yes"),
                );
                let before = browser.pairs();
                let (mut dom, edits, requests) = mount_bid_log(&browser, 1);
                let recover = invalid_auction_recovery(&dom, &edits);
                assert!(requests.borrow().is_empty());
                assert!(browser.writes().is_empty());
                assert_eq!(browser.pairs(), before);

                browser.allow_writes(false);
                dispatch_page_event(&dom, "click", recover, "");
                settle_page(&mut dom);
                assert!(requests.borrow().is_empty());
                assert_eq!(browser.pairs(), before);
                assert!(!browser.writes()[0].accepted);

                browser.allow_writes(true);
                dispatch_page_event(&dom, "click", recover, "");
                settle_page(&mut dom);
                let writes = browser.writes();
                assert_eq!(writes.len(), 3);
                assert_eq!(writes[1].mode, QueryWriteMode::Push);
                assert_eq!(writes[1].keys, ["auction", "lot", "page", "bidlog_page"]);
                assert!(writes[1].pairs.is_empty());
                assert_eq!(writes[2].mode, QueryWriteMode::Replace);
                assert_eq!(browser.value("keep").as_deref(), Some("yes"));
                assert_eq!(browser.value("auction").as_deref(), Some(ROUND_ID));
                assert_eq!(browser.value("lot"), None);
                assert_eq!(requests.borrow().len(), 1);
                assert!(requests.borrow()[0].contains("bid-log?page=1&"));
            }
        });
    }

    #[test]
    fn default_round_replacement_reaches_the_live_state_before_the_body_get() {
        let browser = PageBrowserFixture::new("/bidlog", "page=3&lot=One12");
        let (_dom, _edits, requests) = mount_bid_log(&browser, 1);
        assert_eq!(browser.state().auction_id(), Some(ROUND_ID));
        assert_eq!(browser.state().page, 1);
        assert_eq!(browser.state().bid_log_lot.as_deref(), Some("One12"));
        let writes = browser.writes();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].accepted);
        assert_eq!(writes[0].mode, QueryWriteMode::Replace);
        assert_eq!(writes[0].keys, ["auction", "page", "bidlog_page"]);
        assert_eq!(
            &*requests.borrow(),
            &[format!(
                "/api/v1/auctions/{ROUND_ID}/bid-log?page=1&page_size=20&include=defeated_proxy&world=main&lot=One12"
            )]
        );
    }

    #[test]
    fn automatic_round_replacement_preserves_duplicate_lot_values() {
        let browser = PageBrowserFixture::new("/bidlog", "page=3&lot=One12&lot=Wrong%21+");
        let (_, _, requests) = mount_bid_log(&browser, 1);
        assert_eq!(browser.value("auction").as_deref(), Some(ROUND_ID));
        assert_eq!(browser.value("page"), None);
        assert_eq!(
            browser.pairs(),
            [
                ("lot".to_owned(), "One12".to_owned()),
                ("lot".to_owned(), "Wrong! ".to_owned()),
                ("auction".to_owned(), ROUND_ID.to_owned()),
            ],
        );
        assert!(requests.borrow().is_empty());
    }

    #[test]
    fn rejected_page_size_push_keeps_the_previous_effective_address_state_and_preference() {
        fn pager() -> Element {
            page_test_contexts();
            let query = use_page_query();
            let browser = use_context::<PageBrowserFixture>();
            rsx! {
                PublicPager {
                    page: 2,
                    page_size: 20,
                    total: Some(60),
                    label: "测试出价记录",
                    on_page_change: |_| false,
                    on_page_size_commit: move |size| {
                        browser.allow_writes(false);
                        Some(set_public_page_size(query, size))
                    },
                }
            }
        }

        run_page_events(
            module_path!(),
            "rejected_page_size_push_keeps_the_previous_effective_address_state_and_preference",
            || {
                let browser = PageBrowserFixture::new(
                    "/bidlog",
                    &format!("auction={ROUND_ID}&page=2&lot=One12"),
                );
                let mut dom = VirtualDom::new(pager).with_root_context(browser.clone());
                dom.rebuild_to_vec();
                settle_page(&mut dom);
                let before = browser.state();
                let select = rendered_event_target(&dom, "select", "value", "20", "onchange");
                dispatch_page_event(&dom, "change", select, "50");
                settle_page(&mut dom);

                let writes = browser.writes();
                assert_eq!(writes.len(), 2);
                assert_eq!(writes[0].mode, QueryWriteMode::Replace);
                assert!(writes[0].accepted);
                assert_eq!(writes[1].mode, QueryWriteMode::Push);
                assert!(!writes[1].accepted);
                assert_eq!(browser.value("page_size").as_deref(), Some("20"));
                assert_eq!(browser.value("movement_page_size").as_deref(), Some("20"));
                assert_eq!(browser.value("page").as_deref(), Some("2"));
                assert_eq!(browser.value("auction").as_deref(), Some(ROUND_ID));
                assert_eq!(browser.value("lot").as_deref(), Some("One12"));
                assert_eq!(browser.state(), before);
                assert_eq!(browser.sizes(), [20, 20]);
            },
        );
    }
}
