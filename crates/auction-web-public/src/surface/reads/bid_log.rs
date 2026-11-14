//! 公开竞价日志页。

use auction_web_shared::TruncatedText;
use auction_web_shared::t;
use auction_web_shared::timer::TimeoutFuture;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{
    BidLogEntry, BidLogPage, fetch_bid_log_auctions, fetch_bid_log_page, fetch_worlds, lot_code_for,
};
use crate::surface::{
    BidHistoryParticipant, PublicBoardQuery, PublicPager, QueryWriteMode, Time,
    current_public_pathname, format_credit_amount, public_page_matches, supported_worlds,
    update_public_query_state, use_content_loading, use_public_query_state, world_label,
    write_public_query,
};
use crate::surface::{
    LotDetailLookupButton, LotDetailLookupHost, use_bid_refresh, use_lot_detail_lookup,
};

/// lot 筛选生效的防抖窗口：输入停顿后才应用筛选并无历史记录地写回地址栏，连续输入只发一次
/// 记录请求。
const LOT_FILTER_WRITE_DEBOUNCE_MILLIS: u32 = 300;
/// lot 筛选格式提示的元素 ID，供输入框 `aria-describedby` 引用。
const LOT_FILTER_ERROR_ID: &str = "public-auction-bid-log-filter-error";

pub(crate) fn bid_log() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 世界/拍卖/页码/lot 筛选与地址栏双向同步。
    let query_state = use_public_query_state();
    let mut query_state_signal = query_state.state;
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
    let page_size = use_memo(move || query_state_signal.read().page_size);
    let lot_filter =
        use_memo(move || query_state_signal.read().bid_log_lot.clone().unwrap_or_default());
    // 输入框绑定草稿；合法草稿在输入停顿后才成为已应用筛选（地址栏的 `lot`），记录请求只读后者。
    let lot_draft = use_signal(|| lot_filter.peek().clone());
    let lot_write_generation = use_signal(|| 0_u64);
    // 前进/后退等外部导航改变已应用筛选时，草稿随之同步并作废在途的防抖写入；本页写入的值与
    // 草稿去空白后相同，不覆盖仍在编辑的原文。
    use_effect(move || {
        let applied = lot_filter();
        if lot_draft.peek().trim() != applied {
            reset_lot_draft(lot_draft, lot_write_generation, applied);
        }
    });
    // 迁移期间曾写出 `bidlog_page`；读取后立即无历史记录地收敛回旧版 `page`。
    use_effect(move || {
        if query_state_signal().bid_log_page_alias {
            let mut next = query_state_signal.peek().clone();
            next.bid_log_page_alias = false;
            write_public_query(&next, QueryWriteMode::Replace);
            query_state_signal.set(next);
        }
    });
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec![world_filter()],
    };
    // 生命周期、可见页定时和人工刷新先重读目录；记录请求随后只消费本轮确认的公开场次。
    let auctions = use_resource(move || {
        let generation = refresh_generation();
        let world = world_filter();
        async move {
            let result = fetch_bid_log_auctions(world.clone()).await;
            ((world, generation), result)
        }
    });

    // 保留仍可公开读取的历史选择；仅在所选场次不属于本世界目录时纠偏。
    use_effect(move || {
        let Some(((world, generation), Ok(auction_list))) = &*auctions.read_unchecked() else {
            return;
        };
        // 后台刷新保留旧目录用于展示，但它不能纠偏新世界或新生命周期代际的选择。
        if world != &world_filter() || *generation != refresh_generation() {
            return;
        }
        let selected = selected_auction_id();
        let next_auction_id = auction_list
            .iter()
            .find(|auction| auction.id == selected)
            .or_else(|| auction_list.first())
            .map(|auction| auction.id.clone());
        let next_auction_id = next_auction_id.unwrap_or_default();
        if selected != next_auction_id {
            update_public_query_state(
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

    let bid_log = use_resource(move || {
        let auction_id = selected_auction_id();
        let page = page_number();
        let page_size = page_size();
        let world = world_filter();
        let lot_id = lot_filter().trim().to_owned();
        // 地址栏带入的非法筛选同样不发请求：服务端只会回 400，格式提示由输入框承担。
        let lot_valid = valid_lot_filter(&lot_id);
        let lot_id = (!lot_id.is_empty()).then_some(lot_id);
        let requested = (auction_id.clone(), world.clone(), lot_id.clone(), page, page_size);
        // 不直接订阅全局刷新：先确认本轮目录，避免按过期的场次可见性读取记录。
        // 目录未就绪、失败或已移除该场时不发 GET；查询切换也不能绕过这道门禁。
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
                fetch_bid_log_page(auction_id, page, world, lot_id, page_size).await.map(Some)
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
    let directory_current = auction_snapshot.as_ref().is_some_and(|((world, generation), _)| {
        world == &world_filter() && *generation == refresh_generation()
    });
    use_bid_refresh(bid_log, directory_current && selected_auction.is_some());
    let lookup = use_lot_detail_lookup();
    let current_page = page_number();
    let current_lot_filter = lot_filter();
    let draft_invalid = !valid_lot_filter(lot_draft().trim());
    let on_lot_filter_input = use_callback(move |value: String| {
        // 草稿立即跟随输入，工具条不随请求结果卸载，以保留焦点。非法草稿不发请求，列表保留
        // 已应用筛选的结果，由行内提示说明格式；合法草稿停顿后才应用并写回地址栏。
        let lot = value.trim().to_owned();
        reset_lot_draft(lot_draft, lot_write_generation, value);
        if !valid_lot_filter(&lot) {
            return;
        }
        let generation = *lot_write_generation.peek();
        spawn(async move {
            TimeoutFuture::new(LOT_FILTER_WRITE_DEBOUNCE_MILLIS).await;
            // 迟到任务不能改写已经离开的页面或更新的输入。
            if *lot_write_generation.peek() == generation && current_public_pathname() == "/bidlog"
            {
                apply_lot_filter(query_state_signal, lot);
            }
        });
    });
    let bid_log_state = bid_log.read_unchecked();
    // 同查询后台刷新保留完成快照；目录确认场次已不可见后，迟到记录也不能恢复旧行和分页。
    let current_result = bid_log_state
        .as_ref()
        .filter(|((auction_id, world, lot_id, page, size), _)| {
            selected_auction.is_some()
                && auction_id == &selected_auction_id()
                && world == &world_filter()
                && lot_id.as_deref().unwrap_or_default() == current_lot_filter.trim()
                && *page == current_page
                && *size == page_size()
        })
        .map(|(_, result)| result);
    // 目录或当前查询的记录还没有结果时由顶栏提示加载，正文留空。
    let mut loading = false;
    let body = match (current_auctions, current_result) {
        (Some(Err(_)), _) | (Some(Ok(_)), Some(Err(_))) => rsx! {
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
        (Some(Ok(auction_list)), _) if auction_list.is_empty() => rsx! {
            div { class: "public-auction-empty", role: "status",
                p { class: "public-auction-empty__title", {t!("data-empty")} }
            }
        },
        // 已应用的筛选非法（来自地址栏）时没有发出请求：正文留空且不提示加载，格式提示在输入框旁。
        (Some(Ok(_)), _) if !valid_lot_filter(current_lot_filter.trim()) => rsx! {},
        (Some(Ok(_)), Some(Ok(Some(page))))
            if public_page_matches(page.page, page.page_size, current_page, page_size()) =>
        {
            rsx! {
                BidLogTable {
                    page: page.clone(),
                    current_page,
                    world: world_filter(),
                    lot_filter: current_lot_filter.clone(),
                    on_page_change: move |next_page: i64| {
                        // 从当前状态提交：已应用的 lot 筛选与地址栏同时写入，仍在防抖中的草稿
                        // 不随翻页提交。
                        let mut next = query_state_signal.peek().clone();
                        next.page = next_page;
                        write_public_query(&next, QueryWriteMode::Push);
                        query_state_signal.set(next);
                    },
                    on_page_clamp: move |next_page: i64| {
                        // 纠偏无历史记录写回，不建立可返回的无效页条目。
                        let mut next = query_state_signal.peek().clone();
                        next.page = next_page;
                        write_public_query(&next, QueryWriteMode::Replace);
                        query_state_signal.set(next);
                    },
                    on_page_size_change: move |size| {
                        let mut next = query_state_signal.peek().clone();
                        next.page_size = super::super::normalize_public_page_size(size);
                        next.page = 1;
                        write_public_query(&next, QueryWriteMode::Push);
                        query_state_signal.set(next);
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
        section { class: "public-auction-bid-log-page", aria_label: t!("page-bidlog-title"),
            div { class: "public-auction-bid-log-controls",
                label { r#for: "public-auction-bid-log-world",
                    span { {t!("world-label")} }
                    select {
                        id: "public-auction-bid-log-world",
                        name: "world",
                        autocomplete: "off",
                        value: world_filter(),
                        disabled: world_options.is_empty(),
                        onchange: move |event| {
                            let world = event.value();
                            // 切换世界清除 lot 筛选：草稿一并清空，在途的防抖写入作废。
                            reset_lot_draft(lot_draft, lot_write_generation, String::new());
                            update_public_query_state(
                                query_state_signal,
                                |query| {
                                    // 旧版 Main 使用缺省 URL，非 Main 世界才写入 `world`。
                                    query.world = (world != "main").then_some(world.clone());
                                    query.auction_id = None;
                                    query.bid_log_lot = None;
                                    query.page = 1;
                                },
                                QueryWriteMode::Push,
                            );
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
                        value: selected,
                        disabled: auction_options.is_empty(),
                        onchange: move |event| {
                            let auction_id = event.value();
                            reset_lot_draft(lot_draft, lot_write_generation, String::new());
                            update_public_query_state(
                                query_state_signal,
                                |query| {
                                    query.auction_id = Some(auction_id.clone());
                                    query.bid_log_lot = None;
                                    query.page = 1;
                                },
                                QueryWriteMode::Push,
                            );
                        },
                        for auction in auction_options {
                            option {
                                value: auction.id.clone(),
                                {t!("bidlog-round-option", number: auction.sequence,
                                    status: bid_log_status_label(&auction.status))}
                            }
                        }
                    }
                }
            }
            div { class: "public-auction-bid-log-heading",
                if let Some(auction) = selected_auction {
                    div { class: "public-auction-bid-log-round-header",
                        strong { class: "public-auction-bid-log-round-number",
                            {t!("bidlog-round-number", number: auction.sequence)}
                        }
                        span { class: "public-auction-bid-log-round-world",
                            {t!("bidlog-world", world: super::world_label(&auction.world_id))}
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
                        value: lot_draft(),
                        placeholder: t!("bidlog-filter-placeholder"),
                        aria_invalid: draft_invalid.to_string(),
                        aria_describedby: draft_invalid.then_some(LOT_FILTER_ERROR_ID),
                        oninput: move |event| on_lot_filter_input.call(event.value()),
                    }
                    if !lot_draft().is_empty() {
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                // 清除立即生效，不等防抖。
                                reset_lot_draft(lot_draft, lot_write_generation, String::new());
                                apply_lot_filter(query_state_signal, String::new());
                            },
                            {t!("bidlog-filter-clear")}
                        }
                    }
                    if draft_invalid {
                        p {
                            id: LOT_FILTER_ERROR_ID,
                            class: "public-auction-bid-log-toolbar__error",
                            // 逐字输入 UUID 时每次按键都会判定；用礼貌播报，不打断输入。
                            role: "status",
                            aria_live: "polite",
                            {t!("bidlog-filter-invalid")}
                        }
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
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
) -> Element {
    let has_more = page.has_more.unwrap_or(false);
    let bids = page.bids;
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
                on_page_size_change,
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

/// 去空白后的 lot 筛选能否被公开竞价日志接口接受：空（不筛选）、不超过 32 个 ASCII 字母数字
/// 的编号，或规范小写连字符形式的非 nil UUID。口径与服务端一致，其余取值只会得到 400。
fn valid_lot_filter(value: &str) -> bool {
    if value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return true;
    }
    Uuid::parse_str(value).is_ok_and(|lot_id| {
        let mut buffer = Uuid::encode_buffer();
        let canonical: &str = lot_id.hyphenated().encode_lower(&mut buffer);
        !lot_id.is_nil() && canonical == value
    })
}

/// 把输入草稿换成新值，并作废在途的防抖写入。
fn reset_lot_draft(mut draft: Signal<String>, mut generation: Signal<u64>, value: String) {
    let next = generation.peek().saturating_add(1);
    generation.set(next);
    draft.set(value);
}

/// 把合法的 lot 筛选同时写入已应用状态与地址栏（无历史记录）并回到第一页；筛选未变时
/// 不改动，保留当前页码。
fn apply_lot_filter(mut query_state: Signal<PublicBoardQuery>, lot: String) {
    let lot = (!lot.is_empty()).then_some(lot);
    if query_state.peek().bid_log_lot == lot {
        return;
    }
    let mut next = query_state.peek().clone();
    next.bid_log_lot = lot;
    next.page = 1;
    write_public_query(&next, QueryWriteMode::Replace);
    query_state.set(next);
}

#[cfg(test)]
mod tests {
    use super::valid_lot_filter;

    #[test]
    fn lot_filter_accepts_what_the_bid_log_endpoint_accepts() {
        assert!(valid_lot_filter(""));
        assert!(valid_lot_filter("One12"));
        assert!(valid_lot_filter(&"a".repeat(32)));
        assert!(valid_lot_filter("67e55044-10b1-426f-9247-bb680e5fe0c8"));
    }

    #[test]
    fn lot_filter_rejects_values_the_endpoint_answers_with_400() {
        assert!(!valid_lot_filter(&"a".repeat(33)));
        assert!(!valid_lot_filter("One 12"));
        assert!(!valid_lot_filter("One-12"));
        assert!(!valid_lot_filter("拍品12"));
        // 服务端只接受规范小写连字符形式的非 nil UUID。
        assert!(!valid_lot_filter("67E55044-10B1-426F-9247-BB680E5FE0C8"));
        assert!(!valid_lot_filter("{67e55044-10b1-426f-9247-bb680e5fe0c8}"));
        assert!(!valid_lot_filter("00000000-0000-0000-0000-000000000000"));
    }
}
