//! 公开履约进度页：按已结算场次逐件显示结算后的流程状态（流拍寄回、COD 付款与退回、异世界
//! 交付、卖家货款与成交撤销）。
//!
//! 状态由服务端闭集给出，本页只把状态映射为文案与语气样式，不重建判定；付款期限与 COD 退回
//! 次数只在服务端给出时显示。世界/场次选择、目录门禁与请求身份绑定沿用竞价日志页。重读失败时
//! 缓存层回退的旧快照会连同其确认时刻一起提示，不把旧状态当作当前结果静默显示。

use auction_web_shared::{QueryWriteMode, TruncatedText, format_credit_amount, t};
use dioxus::prelude::*;

use super::lot_filter::{use_lot_filter_draft, valid_lot_filter};
use super::query::{ReadSelection, commit_read_query, update_read_query};
use crate::api::{
    FulfillmentLot, FulfillmentPage, FulfillmentRead, MAX_FULFILLMENT_PAGE_SIZE,
    fetch_fulfillment_auctions, fetch_fulfillment_page, fetch_worlds, lot_code_for,
};
use crate::surface::{
    LotDetailLookupButton, LotDetailLookupHost, PublicPager, format_auction_time,
    normalize_public_page_size, public_page_matches, refresh_public_reads, supported_worlds,
    use_content_loading, use_lot_detail_lookup, use_public_query_state, world_label,
};

/// lot 筛选格式提示的元素 ID，供输入框 `aria-describedby` 引用。
const LOT_FILTER_ERROR_ID: &str = "public-fulfillment-filter-error";

pub(crate) fn fulfillment() -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    // 世界/场次/页码/lot 筛选与地址栏双向同步，键与竞价日志共用。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let world_filter = use_memo(move || {
        query_state_signal
            .read()
            .world
            .clone()
            .filter(|world| matches!(world.as_str(), "main" | "isekai"))
            .unwrap_or_else(|| "main".to_owned())
    });
    let selected_auction_id =
        use_memo(move || query_state_signal.read().auction_id.clone().unwrap_or_default());
    let page_number = use_memo(move || query_state_signal.read().page);
    // 接口每页最多 100 件；共享偏好或地址栏里更大的数量按上限请求，分页器显示实际数量。
    let page_size =
        use_memo(move || query_state_signal.read().page_size.min(MAX_FULFILLMENT_PAGE_SIZE));
    let lot_filter =
        use_memo(move || query_state_signal.read().bid_log_lot.clone().unwrap_or_default());
    let lot_draft = use_lot_filter_draft(query_state_signal, "/fulfillment");
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec![world_filter()],
    };
    // 外壳轮询与人工刷新先重读已结算目录；进度请求随后只消费本轮确认的场次。
    let auctions = use_resource(move || {
        let generation = refresh_generation();
        let world = world_filter();
        async move {
            let result = fetch_fulfillment_auctions(world.clone()).await;
            ((world, generation), result)
        }
    });

    // 保留仍在本世界目录里的选择；所选场次不在目录时纠偏到最近一场。
    use_effect(move || {
        let Some(((world, generation), Ok(auction_list))) = &*auctions.read_unchecked() else {
            return;
        };
        // 后台刷新保留旧目录用于展示，但它不能纠偏新世界或新代际的选择。
        if world != &world_filter() || *generation != refresh_generation() {
            return;
        }
        let selected = selected_auction_id();
        let next_auction_id = auction_list
            .iter()
            .find(|auction| auction.id == selected)
            .or_else(|| auction_list.first())
            .map(|auction| auction.id.clone())
            .unwrap_or_default();
        if selected != next_auction_id {
            update_read_query(
                query_state_signal,
                |query| {
                    query.auction_id =
                        (!next_auction_id.is_empty()).then_some(next_auction_id.clone());
                    query.page = 1;
                },
                QueryWriteMode::Replace,
            );
        }
    });

    let progress = use_resource(move || {
        let auction_id = selected_auction_id();
        let page = page_number();
        let page_size = page_size();
        let world = world_filter();
        let lot = lot_filter().trim().to_owned();
        // 地址栏带入的非法筛选不发请求：服务端只会回 400，格式提示由输入框承担。
        let lot_valid = valid_lot_filter(&lot);
        let lot = (!lot.is_empty()).then_some(lot);
        let requested = (auction_id.clone(), world.clone(), lot.clone(), page, page_size);
        // 不直接订阅全局刷新：目录未就绪、失败或已不含该场时不发 GET。
        let readable = auctions.read_unchecked().as_ref().is_some_and(
            |((loaded_world, generation), result)| {
                loaded_world == &world
                    && *generation == *refresh_generation.peek()
                    && result
                        .as_ref()
                        .is_ok_and(|rows| rows.iter().any(|auction| auction.id == auction_id))
            },
        ) && lot_valid;
        async move {
            let result = if readable {
                fetch_fulfillment_page(auction_id, world, lot, page, page_size).await.map(Some)
            } else {
                Ok(None)
            };
            (requested, result)
        }
    });
    let auction_snapshot = auctions.read_unchecked();
    let current_auctions = auction_snapshot
        .as_ref()
        .filter(|((world, _), _)| world == &world_filter())
        .map(|(_, result)| result);
    let auction_options = match current_auctions {
        Some(Ok(auction_list)) => auction_list.as_slice(),
        _ => &[],
    };
    let selected = selected_auction_id();
    let selected_auction = auction_options.iter().find(|auction| auction.id == selected);
    let lookup = use_lot_detail_lookup();
    let current_page = page_number();
    let current_lot_filter = lot_filter();
    let draft_value = lot_draft.value();
    let draft_invalid = !valid_lot_filter(draft_value.trim());
    let progress_state = progress.read_unchecked();
    // 同查询后台刷新保留完成快照；目录确认场次已不可见后，迟到结果也不能恢复旧行和分页。
    let current_result = progress_state
        .as_ref()
        .filter(|((auction_id, world, lot, page, size), _)| {
            selected_auction.is_some()
                && auction_id == &selected_auction_id()
                && world == &world_filter()
                && lot.as_deref().unwrap_or_default() == current_lot_filter.trim()
                && *page == current_page
                && *size == page_size()
        })
        .map(|(_, result)| result);
    // 目录或当前查询还没有结果时由顶栏提示加载，正文留空。
    let mut loading = false;
    let body = match (current_auctions, current_result) {
        (Some(Err(_)), _) | (Some(Ok(_)), Some(Err(_))) => rsx! {
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
        (Some(Ok(auction_list)), _) if auction_list.is_empty() => rsx! {
            div { class: "public-auction-empty", role: "status",
                p { class: "public-auction-empty__title", {t!("fulfillment-empty-auctions")} }
            }
        },
        // 已应用的筛选非法（来自地址栏）时没有发出请求：正文留空且不提示加载。
        (Some(Ok(_)), _) if !valid_lot_filter(current_lot_filter.trim()) => rsx! {},
        (Some(Ok(_)), Some(Ok(Some(FulfillmentRead { page, stale_since }))))
            if public_page_matches(page.page, page.page_size, current_page, page_size()) =>
        {
            rsx! {
                {fulfillment_stale_notice(stale_since.as_deref())}
                FulfillmentTable {
                    page: page.clone(),
                    current_page,
                    lot_filter: current_lot_filter.clone(),
                    on_page_change: move |next_page: i64| {
                        // 从当前状态提交：仍在防抖中的筛选草稿不随翻页提交。
                        let mut next = query_state_signal.peek().clone();
                        next.page = next_page;
                        commit_read_query(query_state_signal, next, QueryWriteMode::Push);
                    },
                    on_page_clamp: move |next_page: i64| {
                        // 纠偏无历史记录写回，不建立可返回的无效页条目。
                        let mut next = query_state_signal.peek().clone();
                        next.page = next_page;
                        commit_read_query(query_state_signal, next, QueryWriteMode::Replace);
                    },
                    on_page_size_change: move |size| {
                        let mut next = query_state_signal.peek().clone();
                        next.page_size = normalize_public_page_size(size);
                        next.page = 1;
                        commit_read_query(query_state_signal, next, QueryWriteMode::Push);
                    },
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
                        value: world_filter(),
                        disabled: world_options.is_empty(),
                        onchange: move |event| ReadSelection::World(event.value()).select(query_state_signal),
                        for world in &world_options {
                            option { value: world.clone(), {world_label(world)} }
                        }
                    }
                }
                label { r#for: "public-fulfillment-round",
                    span { {t!("fulfillment-round")} }
                    select {
                        id: "public-fulfillment-round",
                        name: "auction",
                        autocomplete: "off",
                        value: selected,
                        disabled: auction_options.is_empty(),
                        onchange: move |event| ReadSelection::Auction(event.value()).select(query_state_signal),
                        for auction in auction_options {
                            option {
                                value: auction.id.clone(),
                                {t!("fulfillment-round-option", number: auction.sequence)}
                            }
                        }
                    }
                }
            }
            div { class: "public-fulfillment__heading",
                if let Some(auction) = selected_auction {
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
                        aria_invalid: draft_invalid.to_string(),
                        aria_describedby: draft_invalid.then_some(LOT_FILTER_ERROR_ID),
                        oninput: move |event| lot_draft.edit(event.value()),
                    }
                    if !draft_value.is_empty() || !current_lot_filter.is_empty() {
                        button {
                            r#type: "button",
                            onclick: move |_| lot_draft.clear(),
                            {t!("fulfillment-filter-clear")}
                        }
                    }
                    if draft_invalid {
                        p {
                            id: LOT_FILTER_ERROR_ID,
                            class: "public-fulfillment__toolbar-error",
                            // 逐字输入 UUID 时每次按键都会判定；用礼貌播报，不打断输入。
                            role: "status",
                            aria_live: "polite",
                            {t!("fulfillment-filter-invalid")}
                        }
                    }
                }
            }
            {body}
            if lookup.failed() {
                p { class: "action-feedback action-feedback--error", role: "alert", {t!("data-error")} }
            }
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
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
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
                on_page_size_change,
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
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::rc::Rc;

    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, LocaleSession, build_bundle};
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{fulfillment_stale_notice, status_key, status_tone};
    use crate::api::FULFILLMENT_STATUSES;
    use crate::surface::{PublicContentLoading, use_content_loading};

    /// 英文译文在编译期内嵌，其余语言的键集合由 i18n 门禁核对与英文一致。
    const ENGLISH: &str = include_str!("../../../i18n/en.ftl");

    type StaleControl = Rc<Cell<Option<Signal<bool>>>>;

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
