//! 公开竞价日志页。

use auction_web_shared::TruncatedText;
use auction_web_shared::t;
use auction_web_shared::validation::restore_select_value;
use auction_web_shared::{QueryWriteMode, Time, format_credit_amount};
use dioxus::prelude::*;

use super::directory::{RoundOptionsError, RoundPage, use_round_directory};
use super::lot_filter::use_lot_filter_draft;
use super::query::{InvalidReadAuction, ReadSelection, use_round_body};
use crate::api::{BidLogEntry, BidLogPage, fetch_bid_log_page, fetch_worlds, lot_code_for};
use crate::surface::read_page_query;
use crate::surface::{
    BidHistoryParticipant, PublicPager, bid_history_rows, public_page_matches, supported_worlds,
    use_content_loading, use_page_query, world_label,
};
use crate::surface::{
    LotDetailLookupButton, LotDetailLookupHost, use_bid_refresh, use_lot_detail_lookup,
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
    let world_filter = use_memo(move || {
        query_state_signal
            .read()
            .world
            .clone()
            .filter(|world| matches!(world.as_str(), "main" | "isekai"))
            .unwrap_or_else(|| "main".to_owned())
    });
    let page_number = use_memo(move || query_state_signal.read().page);
    let page_size = use_memo(move || query_state_signal.read().page_size);
    // 缺省场次与第一页一同提交，无需先替换页码。
    let bounds = use_public_page_bounds(query_state_signal, pathname, |query| {
        query.auction_id().map(|_| (PageKey::BidLog, query.page_size))
    });
    let lot_draft = use_lot_filter_draft(query_state_signal, pathname);
    // 旧页码别名只在仍属于出价记录且可按既定数量读取时原地收敛。
    use_effect(move || {
        let (has_auction, alias, page, page_size) = {
            let query = query_state_signal.read();
            (query.auction_id().is_some(), query.bid_log_page_alias, query.page, query.page_size)
        };
        // 成功替换会写入同一个 Signal，此时前面的读借用必须已释放。
        if has_auction
            && alias
            && public_page_within_offset(page, page_size)
            && page_pathname() == pathname
        {
            let current = read_page_query();
            if current.auction_id().is_some()
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
    let selected_world = world_filter();
    let mut world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec![selected_world.clone()],
    };
    if !world_options.contains(&selected_world) {
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
    use_bid_refresh(data, readable);
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
                    world: world_filter(),
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
                        disabled: world_options.is_empty(),
                        onchange: move |event| {
                            if !ReadSelection::World(event.value()).select(query_state_signal, pathname) {
                                restore_select_value(&event, &selected_world);
                            }
                        },
                        for world in &world_options {
                            option { value: world.clone(), {world_label(world)} }
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
            if lookup.failed() {
                p { class: "action-feedback action-feedback--error", role: "alert", {t!("data-error")} }
            }
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
                        for bid in bids {
                            // 以服务端 bid id 作为稳定 key，避免翻页/筛选刷新后行状态错位。
                            BidLogRow { key: "{bid.id}", bid, world: world.clone() }
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
pub(crate) fn BidLogRow(bid: BidLogEntry, world: String) -> Element {
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
    use std::cell::RefCell;
    use std::rc::Rc;

    use auction_web_shared::QueryWriteMode;
    use dioxus::prelude::*;

    use super::super::query::tests::{
        MountedReadPage, ROUND_ID, invalid_auction_recovery, mount_read_page,
    };
    use super::bid_log;
    use crate::api::BidLogPage;
    use crate::surface::{
        PageBrowserFixture, PageReadFixture, PublicPager, dispatch_page_event, page_test_contexts,
        rendered_event_target, run_page_events, set_public_page_size, settle_page, use_page_query,
    };

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
