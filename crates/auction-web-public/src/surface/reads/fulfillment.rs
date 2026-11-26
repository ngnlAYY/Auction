//! 公开履约进度页：按已结算场次逐件显示结算后的流程状态（流拍寄回、COD 付款与退回、异世界
//! 交付、卖家货款与成交撤销）。
//!
//! 状态由服务端闭集给出，本页只把状态映射为文案与语气样式，不重建判定；付款期限与 COD 退回
//! 次数只在服务端给出时显示。世界/场次选择、目录门禁与请求身份绑定沿用竞价日志页。重读失败时
//! 缓存层回退的旧快照会连同其确认时刻一起提示，不把旧状态当作当前结果静默显示。

use auction_web_shared::validation::restore_select_value;
use auction_web_shared::{QueryWriteMode, TruncatedText, format_credit_amount, t};
use dioxus::prelude::*;

use super::directory::{RoundOptionsError, RoundPage, use_round_directory};
use super::lot_filter::use_lot_filter_draft;
use super::query::{InvalidReadAuction, InvalidReadWorld, ReadSelection, use_round_body};
use crate::api::{
    FulfillmentLot, FulfillmentPage, FulfillmentRead, MAX_FULFILLMENT_PAGE_SIZE,
    fetch_fulfillment_page, lot_code_for,
};
use crate::surface::shell::use_history_world_options;
use crate::surface::{
    LotDetailLookupButton, LotDetailLookupHost, PublicPager, format_auction_time,
    public_page_matches, refresh_public_reads, use_content_loading, use_lot_detail_lookup,
    use_page_query, world_label,
};
use crate::surface::{
    PageKey, PublicPageClampError, page_pathname, set_public_page_size, use_public_page_bounds,
};

/// lot 筛选格式提示的元素 ID，供输入框 `aria-describedby` 引用。
const LOT_FILTER_ERROR_ID: &str = "public-fulfillment-filter-error";

pub(crate) fn fulfillment() -> Element {
    let pathname = RoundPage::Fulfillment.pathname();
    let refresh_generation = use_context::<Signal<u64>>();
    // 世界/场次/页码/lot 筛选与地址栏双向同步，键与竞价日志共用。
    let query_state_signal = use_page_query();
    let world_filter =
        use_memo(move || query_state_signal.read().world.round_world().map(str::to_owned));
    let invalid_world = world_filter.read().is_none();
    let page_number = use_memo(move || query_state_signal.read().page);
    // 接口每页最多 100 件；共享偏好或地址栏里更大的数量按上限请求，分页器显示实际数量。
    let page_size =
        use_memo(move || query_state_signal.read().page_size.min(MAX_FULFILLMENT_PAGE_SIZE));
    let bounds = use_public_page_bounds(query_state_signal, pathname, |query| {
        // 缺省场次的地址纠偏会同时回到第一页。
        query.world.round_world()?;
        query.auction_id().map(|_| (PageKey::Page, query.page_size.min(MAX_FULFILLMENT_PAGE_SIZE)))
    });
    let lot_draft = use_lot_filter_draft(query_state_signal, pathname);

    let selected_world = world_filter().unwrap_or_default();
    let mut world_options = use_history_world_options();
    if !invalid_world && !world_options.contains(&selected_world) {
        world_options.push(selected_world.clone());
    }
    let directory = use_round_directory(
        RoundPage::Fulfillment,
        query_state_signal,
        world_filter,
        refresh_generation,
    );
    let (data, _) = use_round_body(
        directory,
        move || lot_draft.requested_lot().ok(),
        page_number,
        page_size,
        fetch_fulfillment_page,
    );
    let lookup = use_lot_detail_lookup();
    let current_page = page_number();
    let current_lot_filter = lot_draft.applied_value();
    let draft_value = lot_draft.value();
    let filter_error = lot_draft.error(|| t!("fulfillment-filter-invalid"));
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
        _ if bounds.failed(PageKey::Page) => rsx! { PublicPageClampError { bounds } },
        _ if directory.failed() || matches!(current_result, Some(Err(_error))) => rsx! {
            div { class: "public-auction-empty", role: "alert",
                p { class: "public-auction-empty__title", {t!("data-error")} }
                button {
                    class: "public-auction-empty__retry",
                    r#type: "button",
                    // 与看板错误重试同一入口：先使读缓存过期再推进代号，TTL 内的条目也会回到服务端。
                    onclick: move |_| refresh_public_reads(refresh_generation),
                    {t!("data-retry")}
                }
            }
        },
        _ if directory.empty() => rsx! {
            div { class: "public-auction-empty", role: "status",
                p { class: "public-auction-empty__title", {t!("fulfillment-empty-auctions")} }
            }
        },
        // 非法筛选与拒写后的未选场次均可在控件处修正，不进入加载态。
        _ if applied_lot.is_err() || directory.correction_failed() => rsx! {},
        Some(Ok(FulfillmentRead { page, stale_since }))
            if public_page_matches(page.page, page.page_size, current_page, page_size()) =>
        {
            rsx! {
                {fulfillment_stale_notice(stale_since.as_deref())}
                FulfillmentTable {
                    page: page.clone(),
                    current_page,
                    lot_filter: current_lot_filter.clone(),
                    on_page_change: move |page| PageKey::Page.write(query_state_signal, pathname, page, QueryWriteMode::Push),
                    on_page_clamp: move |page| PageKey::Page.write(query_state_signal, pathname, page, QueryWriteMode::Replace),
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
        section { class: "public-fulfillment", aria_label: t!("page-fulfillment-title"),
            div { class: "public-fulfillment__controls",
                label { r#for: "public-fulfillment-world",
                    span { {t!("world-label")} }
                    select {
                        id: "public-fulfillment-world",
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
                // 目录确认为空时没有可选场次，不渲染空的禁用下拉框；选项按需加载，故不以选项数量判断。
                // 链接场次非法时仍渲染（带非法标记），与 Solid 版一致。
                if !directory.empty() || directory.invalid_auction() {
                    label { r#for: "public-fulfillment-round",
                        span { {t!("fulfillment-round")} }
                        select {
                            id: "public-fulfillment-round",
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
                                    {t!("fulfillment-round-option", number: auction.sequence)}
                                }
                            }
                        }
                    }
                }
            }
            if directory.options_failed() && selected_auction.is_some() {
                RoundOptionsError { on_retry: move |_| refresh_public_reads(refresh_generation) }
            }
            div { class: "public-fulfillment__heading",
                if let Some(ref auction) = selected_auction {
                    div { class: "public-fulfillment__round",
                        strong { class: "public-fulfillment__round-number",
                            {t!("fulfillment-round-number", number: auction.sequence)}
                        }
                        span { class: "public-fulfillment__round-detail",
                            {t!("fulfillment-world", world: world_label(&auction.world_id))}
                        }
                        if let Some(settled_at) = auction.settled_at.as_deref() {
                            span { class: "public-fulfillment__round-detail",
                                {t!("fulfillment-settled-at", date: format_auction_time(settled_at))}
                            }
                        }
                    }
                }
                div { class: "public-fulfillment__toolbar",
                    label { r#for: "public-fulfillment-filter", {t!("fulfillment-filter-label")} }
                    input {
                        id: "public-fulfillment-filter",
                        name: "lot",
                        autocomplete: "off",
                        r#type: "search",
                        enterkeyhint: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
                        value: draft_value,
                        placeholder: t!("fulfillment-filter-placeholder"),
                        aria_invalid: filter_error.is_some().to_string(),
                        aria_describedby: filter_error.as_ref().map(|_| LOT_FILTER_ERROR_ID),
                        oninput: move |event| lot_draft.edit(event.value()),
                        onblur: move |_| lot_draft.blur(),
                    }
                    if !draft_value.is_empty() || !current_lot_filter.is_empty() || applied_lot.is_err() {
                        button {
                            r#type: "button",
                            onclick: move |_| lot_draft.clear(),
                            {t!("fulfillment-filter-clear")}
                        }
                    }
                    if let Some(ref error) = filter_error {
                        p { id: LOT_FILTER_ERROR_ID, class: "public-fulfillment__toolbar-error", "{error}" }
                    }
                    span { class: "ui-pager__sr-only", aria_live: "polite", aria_atomic: "true",
                        if let Some(ref error) = filter_error { "{error}" }
                    }
                }
            }
            {body}
            // 抽屉宿主在进度读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。
            LotDetailLookupHost { lookup }
        }
    }
}

fn fulfillment_stale_notice(stale_since: Option<&str>) -> Element {
    rsx! {
        // 播报区域随表格常驻，读屏才会可靠朗读首次出现的旧快照确认时间。
        div {
            class: "public-fulfillment__stale-region",
            role: "status",
            aria_live: "polite",
            aria_atomic: "true",
            if let Some(stale_since) = stale_since {
                p { class: "public-fulfillment__stale",
                    {t!("fulfillment-stale", time: format_auction_time(stale_since))}
                }
            }
        }
    }
}

#[component]
fn FulfillmentTable(
    page: FulfillmentPage,
    current_page: i64,
    lot_filter: String,
    on_page_change: Callback<i64, bool>,
    on_page_clamp: Callback<i64, bool>,
    on_page_size_change: Callback<i64, bool>,
) -> Element {
    let auction_id = page.auction.id.clone();
    let world = page.auction.world.clone();
    rsx! {
        div {
            if page.lots.is_empty() {
                p { class: "public-fulfillment__empty",
                    if lot_filter.is_empty() {
                        {t!("fulfillment-empty")}
                    } else {
                        {t!("fulfillment-filter-empty")}
                    }
                }
            } else {
                div { class: "public-fulfillment__table-wrap",
                    table {
                        class: "public-fulfillment__table",
                        aria_label: t!("page-fulfillment-title"),
                        auction_web_shared::TableColumns {
                            layout: "public-fulfillment",
                            widths: vec![9, 24, 13, 13, 11, 24, 6],
                        }
                        thead {
                            tr {
                                th { scope: "col", {t!("fulfillment-lot")} }
                                th { scope: "col", {t!("catalog-item")} }
                                th { scope: "col", {t!("fulfillment-seller")} }
                                th { scope: "col", {t!("fulfillment-buyer")} }
                                th { scope: "col", class: "public-fulfillment__number", {t!("fulfillment-final-price")} }
                                th { scope: "col", {t!("fulfillment-status")} }
                                th { scope: "col", class: "public-lot-view-cell", {t!("catalog-actions")} }
                            }
                        }
                        tbody {
                            for lot in page.lots {
                                FulfillmentRow {
                                    key: "{lot.lot_id}",
                                    lot,
                                    auction_id: auction_id.clone(),
                                    world: world.clone(),
                                }
                            }
                        }
                    }
                }
            }
            PublicPager {
                page: current_page,
                page_size: page.page_size,
                total: Some(page.total),
                max_page_size: Some(MAX_FULFILLMENT_PAGE_SIZE),
                label: t!("page-fulfillment-title"),
                on_page_change,
                on_page_clamp,
                on_page_size_commit: move |size| Some(on_page_size_change.call(size)),
                report_clamp_failure: true,
            }
        }
    }
}

#[component]
fn FulfillmentRow(lot: FulfillmentLot, auction_id: String, world: String) -> Element {
    let lot_code = lot_code_for(&lot.equipment_category, i64::from(lot.lot_number));
    let buyer = match lot.buyer_name.clone() {
        Some(buyer) => buyer,
        None if lot.status.starts_with("unsold_") => t!("fulfillment-unsold"),
        None => "—".to_owned(),
    };
    let final_price = lot.final_price.map_or_else(|| "—".to_owned(), format_credit_amount);
    let deadline = lot.payment_deadline_at.as_deref().map(
        |deadline| t!("fulfillment-payment-deadline", deadline: format_auction_time(deadline)),
    );
    let returns = lot.cod_return_count.zip(lot.cod_return_limit).map(|(count, limit)| {
        t!("fulfillment-cod-returns", count: count.to_string(), limit: limit.to_string())
    });
    rsx! {
        tr {
            // 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。
            td { class: "public-fulfillment__code", "data-label": t!("fulfillment-lot"), "{lot_code}" }
            td { "data-label": t!("catalog-item"),
                TruncatedText { value: lot.equipment_name.clone(), size: "wide", portal: true }
            }
            td { "data-label": t!("fulfillment-seller"),
                TruncatedText { value: lot.seller_name.clone(), portal: true }
            }
            td { "data-label": t!("fulfillment-buyer"),
                TruncatedText { value: buyer, portal: true }
            }
            td { class: "public-fulfillment__number", "data-label": t!("fulfillment-final-price"),
                "{final_price}"
            }
            td { class: "public-fulfillment__state", "data-label": t!("fulfillment-status"),
                // 状态色只作辅助，文字始终给出完整状态。
                span { class: status_tone(&lot.status), {t!(status_key(&lot.status))} }
                if let Some(deadline) = deadline {
                    span { class: "public-fulfillment__detail", "{deadline}" }
                }
                if let Some(returns) = returns {
                    span { class: "public-fulfillment__detail", "{returns}" }
                }
            }
            td { class: "public-lot-view-cell", "data-label": t!("catalog-actions"),
                LotDetailLookupButton {
                    auction_id,
                    lot_id: lot.lot_id.clone(),
                    lot_code,
                    world: Some(world),
                }
            }
        }
    }
}

/// 状态闭集到译文键；解码器已拒绝闭集外的取值，兜底只为不让页面因新状态崩溃。
fn status_key(status: &str) -> &'static str {
    match status {
        "unsold_return_pending" => "fulfillment-status-unsold-return-pending",
        "unsold_return_sent" => "fulfillment-status-unsold-return-sent",
        "unsold_returned" => "fulfillment-status-unsold-returned",
        "unsold_return_on_hold" => "fulfillment-status-unsold-return-on-hold",
        "payment_mail_pending" => "fulfillment-status-payment-mail-pending",
        "awaiting_payment" => "fulfillment-status-awaiting-payment",
        "payment_overdue" => "fulfillment-status-payment-overdue",
        "payment_on_hold" => "fulfillment-status-payment-on-hold",
        "returned_by_buyer" => "fulfillment-status-returned-by-buyer",
        "returned_on_hold" => "fulfillment-status-returned-on-hold",
        "cod_resend_pending" => "fulfillment-status-cod-resend-pending",
        "cod_resent_awaiting_payment" => "fulfillment-status-cod-resent-awaiting-payment",
        "cod_return_limit_reached" => "fulfillment-status-cod-return-limit-reached",
        "handoff_pending" => "fulfillment-status-handoff-pending",
        "handoff_sent" => "fulfillment-status-handoff-sent",
        "handoff_on_hold" => "fulfillment-status-handoff-on-hold",
        "handoff_returned_by_buyer" => "fulfillment-status-handoff-returned-by-buyer",
        "payout_pending" => "fulfillment-status-payout-pending",
        "payout_sent" => "fulfillment-status-payout-sent",
        "payout_on_hold" => "fulfillment-status-payout-on-hold",
        "completed" => "fulfillment-status-completed",
        "equipment_lost" => "fulfillment-status-equipment-lost",
        "voided_return_pending" => "fulfillment-status-voided-return-pending",
        "voided_return_sent" => "fulfillment-status-voided-return-sent",
        "voided_returned" => "fulfillment-status-voided-returned",
        "voided_on_hold" => "fulfillment-status-voided-on-hold",
        "voided_refund_pending" => "fulfillment-status-voided-refund-pending",
        "voided_relisted" => "fulfillment-status-voided-relisted",
        "voided_relist_pending" => "fulfillment-status-voided-relist-pending",
        "voided_relist_unavailable" => "fulfillment-status-voided-relist-unavailable",
        _ => "fulfillment-status-unknown",
    }
}

/// 状态按语气分组着色：已完成、进行中、需关注、已撤销、未知。
fn status_tone(status: &str) -> &'static str {
    match status {
        "completed" | "unsold_returned" => {
            "public-fulfillment__status public-fulfillment__status--done"
        }
        "unsold_return_pending"
        | "unsold_return_sent"
        | "payment_mail_pending"
        | "awaiting_payment"
        | "cod_resend_pending"
        | "cod_resent_awaiting_payment"
        | "handoff_pending"
        | "handoff_sent"
        | "payout_pending"
        | "payout_sent" => "public-fulfillment__status public-fulfillment__status--progress",
        "unsold_return_on_hold"
        | "payment_overdue"
        | "payment_on_hold"
        | "returned_by_buyer"
        | "returned_on_hold"
        | "cod_return_limit_reached"
        | "handoff_on_hold"
        | "handoff_returned_by_buyer"
        | "payout_on_hold"
        | "equipment_lost" => "public-fulfillment__status public-fulfillment__status--attention",
        status if status.starts_with("voided_") => {
            "public-fulfillment__status public-fulfillment__status--voided"
        }
        _ => "public-fulfillment__status public-fulfillment__status--unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::future::{Future, poll_fn};
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use auction_contracts::PublicFulfillmentAuctionResponse;
    use auction_web_shared::QueryWriteMode;
    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, LocaleSession, build_bundle};
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::super::query::tests::{
        MountedReadPage, OLDER_ROUND_ID, ROUND_ID, invalid_auction_recovery, mount_read_page,
    };
    use super::{fulfillment, fulfillment_stale_notice, status_key, status_tone};
    use crate::api::{AuctionListPage, FULFILLMENT_STATUSES, FulfillmentPage, HistoricalAuction};
    use crate::surface::reads::tests::{dispatch_select_event, select_values};
    use crate::surface::shell::WorldCatalog;
    use crate::surface::{
        PageBrowserFixture, PageReadFixture, PublicContentLoading, dispatch_page_event,
        page_test_contexts, rendered_event_target, run_page_events, settle_page,
        use_content_loading,
    };

    /// 英文译文在编译期内嵌，其余语言的键集合由 i18n 门禁核对与英文一致。
    const ENGLISH: &str = include_str!("../../../i18n/en.ftl");

    type StaleControl = Rc<Cell<Option<Signal<bool>>>>;

    fn fulfillment_page() -> Element {
        page_test_contexts();
        use_context_provider(WorldCatalog::new);
        fulfillment()
    }

    const MAIN_FIRST: &str = "/api/v1/auctions?page=1&page_size=200&status=settled&world=main";
    const MAIN_SECOND: &str = "/api/v1/auctions?page=2&page_size=200&status=settled&world=main";
    const MAIN_THIRD: &str = "/api/v1/auctions?page=3&page_size=200&status=settled&world=main";
    const ISEKAI_FIRST: &str = "/api/v1/auctions?page=1&page_size=200&status=settled&world=isekai";
    const ISEKAI_ROUND: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2d";

    type DirectoryReply = Pin<Box<dyn Future<Output = Result<AuctionListPage, String>>>>;

    #[derive(Default)]
    struct FulfillmentReadTrace {
        urls: RefCell<Vec<String>>,
        second_page: Cell<Option<Waker>>,
        released: Cell<bool>,
    }

    impl FulfillmentReadTrace {
        fn mount(selected: Option<&str>) -> (VirtualDom, Rc<Self>) {
            let search = selected.map_or_default(|id| format!("auction={id}"));
            let browser = PageBrowserFixture::new("/fulfillment", &search);
            let trace = Rc::new(Self::default());
            // 两个响应类型在共同的公开读取边界记账，应用层缓存无法掩盖重复调用。
            let directory = PageReadFixture::new({
                let trace = trace.clone();
                move |url: String| -> DirectoryReply {
                    trace.urls.borrow_mut().push(url.clone());
                    let trace = trace.clone();
                    Box::pin(async move {
                        if url == MAIN_SECOND {
                            poll_fn(|cx| {
                                if trace.released.get() {
                                    Poll::Ready(())
                                } else {
                                    trace.second_page.set(Some(cx.waker().clone()));
                                    Poll::Pending
                                }
                            })
                            .await;
                        }
                        Ok(directory_reply(&url))
                    })
                }
            });
            let body = PageReadFixture::new({
                let trace = trace.clone();
                move |url: String| {
                    let (id, world) =
                        [(ROUND_ID, "main"), (OLDER_ROUND_ID, "main"), (ISEKAI_ROUND, "isekai")]
                            .into_iter()
                            .find(|(id, world)| url == fulfillment_url(id, world))
                            .expect("正文请求属于已确认的场次");
                    trace.urls.borrow_mut().push(url);
                    Ok::<_, String>(FulfillmentPage {
                        auction: PublicFulfillmentAuctionResponse {
                            id: id.to_owned(),
                            world: world.to_owned(),
                            number: 1,
                        },
                        lots: Vec::new(),
                        page: 1,
                        page_size: 20,
                        total: 0,
                    })
                }
            });
            let mut dom = VirtualDom::new(fulfillment_page)
                .with_root_context(browser)
                .with_root_context(directory)
                .with_root_context(body);
            dom.rebuild_in_place();
            settle_page(&mut dom);
            (dom, trace)
        }

        fn complete_second_page(&self, dom: &mut VirtualDom) {
            self.released.set(true);
            self.second_page.take().expect("主世界目录第 2 页正在读取").wake();
            settle_page(dom);
        }
    }

    fn directory_reply(url: &str) -> AuctionListPage {
        let (page, world, id, sequence, row_count, total) = match url {
            MAIN_FIRST => (1, "main", ROUND_ID, 401, 200, 401),
            MAIN_SECOND => (2, "main", OLDER_ROUND_ID, 201, 200, 401),
            MAIN_THIRD => (3, "main", "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2e", 1, 1, 401),
            ISEKAI_FIRST => (1, "isekai", ISEKAI_ROUND, 1, 1, 1),
            _ => panic!("意外的目录请求 {url}"),
        };
        let auctions = (0..row_count)
            .map(|offset| {
                let sequence = sequence - offset;
                HistoricalAuction {
                    id: if offset == 0 {
                        id.to_owned()
                    } else {
                        format!("00000000-0000-4000-8000-{sequence:012x}")
                    },
                    world_id: world.to_owned(),
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
                }
            })
            .collect();
        AuctionListPage {
            auctions,
            total: Some(total),
            page: Some(page),
            page_size: 200,
            cursor: None,
            next_cursor: None,
            has_more: None,
        }
    }

    fn fulfillment_url(id: &str, world: &str) -> String {
        format!("/api/v1/auctions/{id}/fulfillment?page=1&page_size=20&world={world}")
    }

    fn focus_round(dom: &mut VirtualDom) {
        let select =
            rendered_event_target(dom, "select", "id", "public-fulfillment-round", "onfocus");
        dispatch_select_event(dom, select, None);
        settle_page(dom);
    }

    #[test]
    fn focusing_fulfillment_rounds_continues_once_from_the_next_unread_page() {
        run_page_events(
            module_path!(),
            "focusing_fulfillment_rounds_continues_once_from_the_next_unread_page",
            || {
                let (mut dom, trace) = FulfillmentReadTrace::mount(None);
                let body = fulfillment_url(ROUND_ID, "main");
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, &body]);
                focus_round(&mut dom);
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, &body, MAIN_SECOND]);
                focus_round(&mut dom);
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, &body, MAIN_SECOND]);
                trace.complete_second_page(&mut dom);
                focus_round(&mut dom);
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, &body, MAIN_SECOND, MAIN_THIRD]);
            },
        );
    }

    #[test]
    fn focusing_after_locating_a_deep_fulfillment_round_only_reads_later_pages() {
        run_page_events(
            module_path!(),
            "focusing_after_locating_a_deep_fulfillment_round_only_reads_later_pages",
            || {
                let (mut dom, trace) = FulfillmentReadTrace::mount(Some(OLDER_ROUND_ID));
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, MAIN_SECOND]);
                trace.complete_second_page(&mut dom);
                let body = fulfillment_url(OLDER_ROUND_ID, "main");
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, MAIN_SECOND, &body]);
                focus_round(&mut dom);
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, MAIN_SECOND, &body, MAIN_THIRD]);
            },
        );
    }

    #[test]
    fn switching_fulfillment_worlds_discards_the_pending_old_directory_page() {
        run_page_events(
            module_path!(),
            "switching_fulfillment_worlds_discards_the_pending_old_directory_page",
            || {
                let (mut dom, trace) = FulfillmentReadTrace::mount(Some(ROUND_ID));
                let main_body = fulfillment_url(ROUND_ID, "main");
                focus_round(&mut dom);
                assert_eq!(&*trace.urls.borrow(), &[MAIN_FIRST, &main_body, MAIN_SECOND]);
                let world = rendered_event_target(
                    &dom,
                    "select",
                    "id",
                    "public-fulfillment-world",
                    "onchange",
                );
                dispatch_select_event(&dom, world, Some("isekai"));
                settle_page(&mut dom);
                let isekai_body = fulfillment_url(ISEKAI_ROUND, "isekai");
                let expected = [MAIN_FIRST, &main_body, MAIN_SECOND, ISEKAI_FIRST, &isekai_body];
                assert_eq!(&*trace.urls.borrow(), &expected);
                trace.complete_second_page(&mut dom);
                focus_round(&mut dom);
                assert_eq!(&*trace.urls.borrow(), &expected);
                assert_eq!(
                    select_values(&dom, "id", "public-fulfillment-round"),
                    ["", ISEKAI_ROUND]
                );
            },
        );
    }

    fn mount_fulfillment(
        browser: &PageBrowserFixture,
        expected_auction: &'static str,
        expected_page: i64,
    ) -> MountedReadPage {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let request_log = requests.clone();
        let applied = browser.clone();
        let body = PageReadFixture::new(move |url: String| {
            assert_eq!(applied.value("auction").as_deref(), Some(expected_auction));
            assert_eq!(
                applied.value("page"),
                (expected_page > 1).then(|| expected_page.to_string())
            );
            request_log.borrow_mut().push(url);
            Ok::<_, String>(FulfillmentPage {
                auction: PublicFulfillmentAuctionResponse {
                    id: expected_auction.to_owned(),
                    world: "main".to_owned(),
                    number: if expected_auction == ROUND_ID { 2 } else { 1 },
                },
                lots: Vec::new(),
                page: expected_page,
                page_size: 20,
                total: 40,
            })
        });
        let (dom, edits) = mount_read_page(fulfillment_page, browser, body);
        (dom, edits, requests)
    }

    #[test]
    fn invalid_fulfillment_world_blocks_reads_until_an_explicit_selection() {
        run_page_events(
            module_path!(),
            "invalid_fulfillment_world_blocks_reads_until_an_explicit_selection",
            || {
                for world in ["world=unknown", "world=main&world=isekai"] {
                    let browser =
                        PageBrowserFixture::new("/fulfillment", &format!("{world}&keep=yes"));
                    let (mut dom, _, requests) = mount_fulfillment(&browser, ROUND_ID, 1);
                    assert!(requests.borrow().is_empty());
                    assert!(browser.writes().is_empty());
                    let select = rendered_event_target(
                        &dom,
                        "select",
                        "id",
                        "public-fulfillment-world",
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
                    assert!(!browser.state().world.invalid());
                    assert_eq!(browser.value("keep").as_deref(), Some("yes"));
                    assert!(requests.borrow()[0].ends_with("world=main"));
                }
            },
        );
    }

    #[test]
    fn invalid_auction_requires_an_explicit_fulfillment_selection() {
        run_page_events(
            module_path!(),
            "invalid_auction_requires_an_explicit_fulfillment_selection",
            || {
                for auction in [
                    "auction=bad".to_owned(),
                    format!("auction={ROUND_ID}&auction={OLDER_ROUND_ID}"),
                ] {
                    let browser = PageBrowserFixture::new(
                        "/fulfillment",
                        &format!("{auction}&page=700&lot=One12&keep=yes"),
                    );
                    let before = browser.pairs();
                    let (mut dom, edits, requests) = mount_fulfillment(&browser, OLDER_ROUND_ID, 1);
                    invalid_auction_recovery(&dom, &edits);
                    assert_eq!(browser.pairs(), before);
                    assert!(browser.writes().is_empty());
                    assert!(requests.borrow().is_empty());

                    let select =
                        rendered_event_target(&dom, "select", "name", "auction", "onchange");
                    dispatch_page_event(&dom, "change", select, OLDER_ROUND_ID);
                    settle_page(&mut dom);
                    let writes = browser.writes();
                    assert_eq!(writes.len(), 1);
                    assert_eq!(writes[0].mode, QueryWriteMode::Push);
                    assert_eq!(writes[0].keys, ["auction", "lot", "page"]);
                    assert_eq!(
                        writes[0].pairs,
                        [("auction".to_owned(), OLDER_ROUND_ID.to_owned())]
                    );
                    assert_eq!(
                        browser.pairs(),
                        [
                            ("keep".to_owned(), "yes".to_owned()),
                            ("auction".to_owned(), OLDER_ROUND_ID.to_owned()),
                        ]
                    );
                    assert_eq!(
                        browser.state().auction_parameter().selected_id(),
                        Some(OLDER_ROUND_ID)
                    );
                    assert_eq!(
                        &*requests.borrow(),
                        &[format!(
                            "/api/v1/auctions/{OLDER_ROUND_ID}/fulfillment?page=1&page_size=20&world=main"
                        )]
                    );
                }
            },
        );
    }

    #[test]
    fn fulfillment_keeps_the_selected_round_or_applies_the_default() {
        for (search, expected, page, write_count) in [
            (format!("auction={OLDER_ROUND_ID}&page=2&lot=One12"), OLDER_ROUND_ID, 2, 0),
            ("page=2&lot=One12".to_owned(), ROUND_ID, 1, 1),
        ] {
            let browser = PageBrowserFixture::new("/fulfillment", &search);
            let (_, _, requests) = mount_fulfillment(&browser, expected, page);
            assert_eq!(browser.writes().len(), write_count);
            assert_eq!(browser.value("lot").as_deref(), Some("One12"));
            assert_eq!(
                &*requests.borrow(),
                &[format!(
                    "/api/v1/auctions/{expected}/fulfillment?page={page}&page_size=20&world=main&lot=One12"
                )]
            );
        }
    }

    #[component]
    fn StaleNoticeHarness() -> Element {
        let bundle = try_use_init_i18n_from_bundle(|| {
            build_bundle(&DEFAULT_PUBLIC_LOCALE, ENGLISH, &DEFAULT_PUBLIC_LOCALE, None)
        })
        .expect("初始化英文译文");
        use_context_provider(|| LocaleSession::new(bundle, DEFAULT_PUBLIC_LOCALE));
        use_context_provider(|| PublicContentLoading(Signal::new(BTreeSet::new())));
        let stale = use_signal(|| false);
        consume_context::<StaleControl>().set(Some(stale));
        let notice = fulfillment_stale_notice(stale().then_some("2026-01-02T03:04:05Z"));
        use_content_loading(false);
        notice
    }

    #[test]
    fn stale_notice_can_appear_and_disappear_on_the_same_render_scope() {
        let control = StaleControl::default();
        let mut dom = VirtualDom::new(StaleNoticeHarness).with_root_context(control.clone());
        dom.rebuild_in_place();
        let mut stale = control.get().expect("挂载履约提示");
        dom.in_runtime(|| stale.set(true));
        let edits = dom.render_immediate_to_vec().edits;
        assert!(edits.iter().any(|edit| matches!(
            edit,
            Mutation::CreateTextNode { value, .. } if value.contains("02 Jan 2026, 11:04:05")
        )));

        dom.in_runtime(|| stale.set(false));
        let edits = dom.render_immediate_to_vec().edits;
        assert!(edits.iter().any(|edit| matches!(edit, Mutation::ReplaceWith { .. })));
        assert_eq!(control.get(), Some(stale));
    }

    #[test]
    fn every_status_maps_to_its_own_translated_label() {
        let mut keys = Vec::new();
        for status in FULFILLMENT_STATUSES {
            let key = status_key(status);
            assert_eq!(key, format!("fulfillment-status-{}", status.replace('_', "-")));
            assert!(
                ENGLISH.lines().any(|line| line.split_once(" =").is_some_and(|(id, _)| id == key)),
                "{key} 缺少英文译文"
            );
            keys.push(key);
        }
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), FULFILLMENT_STATUSES.len(), "每个状态一个文案");
        assert_eq!(status_key("future_status"), "fulfillment-status-unknown");
    }

    #[test]
    fn every_status_has_a_tone_group() {
        for status in FULFILLMENT_STATUSES {
            let tone = status_tone(status);
            assert!(tone.starts_with("public-fulfillment__status public-fulfillment__status--"));
            // 只有“未知”本身落到未知分组；其余状态漏配会在这里暴露。
            assert_eq!(tone.ends_with("--unknown"), status == "unknown", "{status}");
        }
        assert!(status_tone("completed").ends_with("--done"));
        assert!(status_tone("payment_overdue").ends_with("--attention"));
        assert!(status_tone("awaiting_payment").ends_with("--progress"));
        for status in FULFILLMENT_STATUSES.iter().filter(|status| status.starts_with("voided_")) {
            assert!(status_tone(status).ends_with("--voided"), "{status}");
        }
    }
}
