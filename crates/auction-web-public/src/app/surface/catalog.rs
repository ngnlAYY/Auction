//! Public 拍卖目录、标的行、倒计时和展示映射组件。

use super::*;

#[component]
pub(crate) fn CatalogTable(
    rows: Vec<CatalogRow>,
    now: DateTime<Utc>,
    projection_kind: String,
    category_counts: BTreeMap<String, i64>,
    mut active_category: Signal<String>,
    mut page_number: Signal<i64>,
    page_total: i64,
    page_size: i64,
    can_bid: bool,
    allow_slow: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    let active = active_category();
    let groups = grouped_catalog_rows(&rows, &active);
    let is_live = projection_kind == "live";
    let all_count = catalog_count(&category_counts, "all", &rows);

    rsx! {
        nav { class: "public-auction-filter", aria_label: t!("filter-label"),
            button {
                class: "public-auction-filter__tab",
                r#type: "button",
                aria_pressed: active == "all",
                onclick: move |_| {
                    active_category.set("all".to_owned());
                    page_number.set(1);
                    update_public_query(
                        |query| {
                            query.category = None;
                            query.page = 1;
                        },
                        false,
                    );
                },
                {t!("filter-all")}
                span { class: "public-auction-filter__count", "{all_count}" }
            }
            for (category, _) in CATALOG_CATEGORIES {
                button {
                    class: "public-auction-filter__tab",
                    r#type: "button",
                    aria_pressed: active == category,
                    onclick: move |_| {
                        active_category.set(category.to_owned());
                        page_number.set(1);
                        update_public_query(
                            |query| {
                                query.category = Some(category.to_owned());
                                query.page = 1;
                            },
                            false,
                        );
                    },
                    {catalog_category_label(category)}
                    span { class: "public-auction-filter__count", "{catalog_count(&category_counts, category, &rows)}" }
                }
            }
        }
        if groups.is_empty() {
            div { class: "public-auction-empty",
                h2 { {t!("catalog-empty")} }
            }
        }
        for (category, category_rows) in groups {
            section { class: "public-auction-category",
                h2 { class: "public-auction-category__title",
                    {catalog_category_label(category)}
                    span { class: "public-auction-category__code", "{catalog_category_code(category)}" }
                }
                if is_live {
                    LiveCatalogTable {
                        rows: category_rows,
                        now,
                        can_bid,
                        allow_slow,
                        authenticated,
                        viewer_name: viewer_name.clone(),
                    }
                } else {
                    PreviewCatalogTable { rows: category_rows }
                }
            }
        }
        PublicPager {
            page: page_number(),
            page_size,
            total: Some(page_total),
            has_more: false,
            label: t!("catalog-pagination"),
            on_page_change: move |page| {
                page_number.set(page);
                update_public_query(|query| query.page = page, false);
            },
        }
    }
}
#[component]
pub(crate) fn LiveCatalogTable(
    rows: Vec<CatalogRow>,
    now: DateTime<Utc>,
    can_bid: bool,
    allow_slow: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("catalog-code")} }
                        th { {t!("catalog-item")} }
                        th { {t!("catalog-level")} }
                        th { {t!("catalog-seller")} }
                        th { class: "public-auction-catalog__number", {t!("catalog-current")} }
                        th { class: "public-auction-catalog__number", {t!("catalog-next-bid")} }
                        th { {t!("catalog-leader")} }
                        th { {t!("catalog-remaining")} }
                        th { {t!("catalog-status")} }
                        th { span { class: "public-visually-hidden", {t!("catalog-actions")} } }
                    }
                }
                tbody {
                    for row in rows {
                        LiveCatalogRow {
                            row,
                            now,
                            can_bid,
                            allow_slow,
                            authenticated,
                            viewer_name: viewer_name.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn PreviewCatalogTable(rows: Vec<CatalogRow>) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("catalog-code")} }
                        th { {t!("catalog-item")} }
                        th { {t!("catalog-level")} }
                        th { {t!("catalog-seller")} }
                        th { class: "public-auction-catalog__number", {t!("catalog-start-price")} }
                        th { {t!("catalog-leader")} }
                        th { {t!("catalog-remaining")} }
                        th { {t!("catalog-status")} }
                        th { span { class: "public-visually-hidden", {t!("catalog-actions")} } }
                    }
                }
                tbody {
                    for row in rows {
                        PreviewCatalogRow { row }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn LiveCatalogRow(
    row: CatalogRow,
    now: DateTime<Utc>,
    can_bid: bool,
    allow_slow: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let is_leading = viewer_name
        .as_deref()
        .zip(row.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    let leading = if is_leading { "public-auction-catalog__leading" } else { "" };
    let countdown = if row.status == "live" {
        countdown_display(&row.remaining, now)
    } else {
        CountdownDisplay { label: t!("value-empty"), urgent: false, expired: false, empty: true }
    };
    let countdown_css_class = countdown_class(&countdown);
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let leader = row.leader_name.clone().unwrap_or_else(|| t!("value-empty"));
    let seller = row.seller_name.clone();
    let current_price = format_amount(row.current_price);
    let next_bid = format_amount(row.next_bid);

    rsx! {
        tr { class: "{leading}",
            td { class: "public-auction-catalog__code", "data-label": t!("catalog-code"),
                a { href: lot_detail_href(&row.lot_id), "{row.code}" }
            }
            td { class: "public-auction-catalog__name", "data-label": t!("catalog-item"),
                span { class: "public-auction-catalog__item-name", title: row.name.clone(), "{row.name}" }
                if let Some(avg_roll) = row.avg_roll {
                    span { class: "public-auction-catalog__roll", "{avg_roll:.1}%" }
                }
            }
            td { "data-label": t!("catalog-level"), "{level}" }
            td { "data-label": t!("catalog-seller"),
                span { class: "public-auction-catalog__seller", title: seller.clone(), "{seller}" }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-current"),
                "{current_price}"
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-next-bid"),
                "{next_bid}"
            }
            td { class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                if row.leader_name.is_some() {
                    CatalogParticipant {
                        name: leader,
                        marker: None,
                        trailing: is_leading.then(|| t!("catalog-you")),
                    }
                } else {
                    span { class: "public-auction-catalog__none", "{leader}" }
                }
            }
            td {
                class: "{countdown_css_class}",
                "data-label": t!("catalog-remaining"),
                title: if countdown.empty { "".to_owned() } else { row.remaining.clone() },
                "{countdown.label}"
            }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                LiveCatalogActions {
                    lot_id: row.lot_id.clone(),
                    lot_code: row.code.clone(),
                    can_bid: can_bid && row.status == "live",
                    current_price: row.current_price,
                    next_bid: row.next_bid,
                    lot_status: row.status.clone(),
                    allow_slow,
                    authenticated,
                    leading: is_leading,
                    show_view: false,
                    show_proxy_summary: false,
                }
            }
        }
    }
}

#[component]
pub(crate) fn LiveCatalogActions(
    lot_id: String,
    lot_code: String,
    can_bid: bool,
    current_price: i64,
    next_bid: i64,
    lot_status: String,
    allow_slow: bool,
    authenticated: bool,
    leading: bool,
    show_view: bool,
    show_proxy_summary: bool,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let requested_lot_id = lot_id.clone();
    let proxy_state = use_resource(move || {
        let _ = refresh_generation();
        let lot_id = requested_lot_id.clone();
        async move { if authenticated { fetch_my_proxy(lot_id).await } else { Ok(None) } }
    });
    let proxy_snapshot = proxy_state.read_unchecked().clone();
    let active_proxy = match proxy_snapshot.as_ref() {
        Some(Ok(Some(proxy))) if proxy.status == "active" => Some(proxy),
        _ => None,
    };
    let proxy_leading =
        active_proxy.and_then(|proxy| proxy.bid.as_ref()).is_some_and(|bid| bid.leading);
    let effective_leading = leading || proxy_leading;
    let ordinary_blocked = effective_leading || active_proxy.is_some();
    let proxy_action_enabled =
        active_proxy.map_or(!effective_leading, |proxy| proxy.capabilities.can_lower);
    let action_reason = if !can_bid {
        t!("bid-unavailable")
    } else if active_proxy.is_some() && !effective_leading {
        t!("proxy-active")
    } else if effective_leading {
        t!("bid-leading")
    } else if !proxy_action_enabled {
        t!("proxy-cannot-lower")
    } else {
        String::new()
    };
    let action_reason_id = format!("public-auction-action-reason-{lot_id}");
    let mut action = use_signal(|| None::<String>);
    let mut proxy_success = use_signal(|| None::<Box<ProxyCommandAuthorization>>);
    let action_name = action();
    let bid_lot_id = lot_id.clone();
    let proxy_lot_id = lot_id.clone();
    let bid_lot_status = lot_status.clone();
    let proxy_lot_status = lot_status.clone();
    let bid_dialog_title = t!("bid-dialog-title", code: lot_code.clone());
    let proxy_dialog_title = t!("proxy-dialog-title", code: lot_code);

    rsx! {
        span { class: "public-auction-catalog__actions-list",
            if show_view {
                a { href: lot_detail_href(&lot_id), {t!("catalog-view")} }
            }
            if authenticated && (can_bid || show_proxy_summary) {
                button {
                    r#type: "button",
                    disabled: !can_bid || ordinary_blocked,
                    aria_describedby: if !can_bid || ordinary_blocked {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        proxy_success.set(None);
                        action.set(if action() == Some("bid".to_owned()) {
                            None
                        } else {
                            Some("bid".to_owned())
                        });
                    },
                    {t!("catalog-bid")}
                }
                button {
                    r#type: "button",
                    disabled: !can_bid || !proxy_action_enabled,
                    aria_describedby: if !can_bid || !proxy_action_enabled {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        proxy_success.set(None);
                        action.set(if action() == Some("proxy".to_owned()) {
                            None
                        } else {
                            Some("proxy".to_owned())
                        });
                    },
                    if active_proxy.is_some() {
                        {t!("catalog-lower-proxy")}
                    } else {
                        {t!("catalog-proxy")}
                    }
                }
            } else if can_bid {
                Link { to: Route::Login {}, {t!("login-submit")} }
            } else if show_proxy_summary {
                button {
                    r#type: "button",
                    disabled: true,
                    aria_describedby: action_reason_id.clone(),
                    {t!("catalog-bid")}
                }
                button {
                    r#type: "button",
                    disabled: true,
                    aria_describedby: action_reason_id.clone(),
                    {t!("catalog-proxy")}
                }
            }
            if (show_proxy_summary && !can_bid) || ordinary_blocked || !proxy_action_enabled {
                span {
                    id: action_reason_id,
                    class: "public-auction-catalog__leading-reason",
                    role: "note",
                    "{action_reason}"
                }
            }
        }
        if show_proxy_summary && authenticated {
            proxy::ProxySummary {
                snapshot: proxy_snapshot.clone(),
                refresh_generation,
            }
        }
        if action_name == Some("bid".to_owned()) {
            div {
                class: "public-auction-dialog",
                role: "dialog",
                aria_modal: "true",
                aria_label: bid_dialog_title.clone(),
                onclick: move |_| action.set(None),
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        action.set(None);
                    }
                },
                div {
                    class: "public-auction-dialog__panel",
                    onclick: move |event| event.stop_propagation(),
                    div { class: "public-auction-dialog__header",
                        h2 { "{bid_dialog_title}" }
                        button {
                            class: "public-auction-dialog__close",
                            r#type: "button",
                            onclick: move |_| action.set(None),
                            aria_label: t!("catalog-close"),
                            {t!("catalog-close")}
                        }
                    }
                    {commands::bid_form(commands::BidFormInput {
                        lot_id: bid_lot_id,
                        current_price,
                        next_bid,
                        lot_status: bid_lot_status,
                        can_bid,
                        blocked: ordinary_blocked,
                        blocked_reason: action_reason.clone(),
                        on_accepted: EventHandler::new(move |_: ()| action.set(None)),
                        on_close: EventHandler::new(move |_: ()| action.set(None)),
                    })}
                }
            }
        }
        if action_name == Some("proxy".to_owned()) {
            div {
                class: "public-auction-dialog",
                role: "dialog",
                aria_modal: "true",
                aria_label: proxy_dialog_title.clone(),
                onclick: move |_| {
                    proxy_success.set(None);
                    action.set(None);
                },
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        proxy_success.set(None);
                        action.set(None);
                    }
                },
                div {
                    class: "public-auction-dialog__panel",
                    onclick: move |event| event.stop_propagation(),
                    div { class: "public-auction-dialog__header",
                        h2 { "{proxy_dialog_title}" }
                        button {
                            class: "public-auction-dialog__close",
                            r#type: "button",
                            onclick: move |_| {
                                proxy_success.set(None);
                                action.set(None);
                            },
                            aria_label: t!("catalog-close"),
                            {t!("catalog-close")}
                        }
                    }
                    {commands::proxy_form(commands::ProxyFormInput {
                        lot_id: proxy_lot_id,
                        current_price,
                        next_bid,
                        lot_status: proxy_lot_status,
                        allow_slow,
                        leading: effective_leading,
                        blocked_reason: action_reason,
                        proxy: proxy_snapshot,
                        refresh_generation,
                        saved_authorization: proxy_success,
                        on_close: EventHandler::new(move |_: ()| {
                            proxy_success.set(None);
                            action.set(None);
                        }),
                    })}
                }
            }
        }
    }
}

/// 公开拍卖参与者标识；长名称保持可读的省略样式，并保留旧版的代理/当前用户标记。
#[component]
pub(crate) fn CatalogParticipant(
    name: String,
    marker: Option<String>,
    trailing: Option<String>,
) -> Element {
    rsx! {
        span { class: "public-auction-participant-identity",
            if let Some(marker) = marker {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--leading",
                    "{marker}"
                }
            }
            span {
                class: "public-auction-participant-identity__name",
                title: name.clone(),
                "{name}"
            }
            if let Some(trailing) = trailing {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                    "{trailing}"
                }
            }
        }
    }
}

#[component]
pub(crate) fn PreviewCatalogRow(row: CatalogRow) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let seller = row.seller_name.clone();
    let start_price = format_amount(row.start_price);

    rsx! {
        tr {
            td { class: "public-auction-catalog__code", "data-label": t!("catalog-code"),
                a { href: lot_detail_href(&row.lot_id), "{row.code}" }
            }
            td { class: "public-auction-catalog__name", "data-label": t!("catalog-item"),
                span { class: "public-auction-catalog__item-name", title: row.name.clone(), "{row.name}" }
                if let Some(avg_roll) = row.avg_roll {
                    span { class: "public-auction-catalog__roll", "{avg_roll:.1}%" }
                }
            }
            td { "data-label": t!("catalog-level"), "{level}" }
            td { "data-label": t!("catalog-seller"),
                span { class: "public-auction-catalog__seller", title: seller.clone(), "{seller}" }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-start-price"),
                "{start_price}"
            }
            td { class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                span { class: "public-auction-catalog__none", {t!("value-empty")} }
            }
            td { class: "public-auction-countdown--none", "data-label": t!("catalog-remaining"), {t!("value-empty")} }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                span { class: "public-auction-catalog__actions-list",
                    a { href: lot_detail_href(&row.lot_id), {t!("catalog-view")} }
                }
            }
        }
    }
}

pub(crate) fn grouped_catalog_rows(
    rows: &[CatalogRow],
    active: &str,
) -> Vec<(&'static str, Vec<CatalogRow>)> {
    let mut groups: Vec<(&str, Vec<CatalogRow>)> =
        CATALOG_CATEGORIES.iter().map(|(category, _)| (*category, Vec::new())).collect();
    let mut other = Vec::new();

    for row in rows {
        let category = catalog_category_key(&row.category);
        if active != "all" && active != category {
            continue;
        }
        if let Some((_, entries)) = groups.iter_mut().find(|(key, _)| *key == category) {
            entries.push(row.clone());
        } else if active == "all" || active == "other" {
            other.push(row.clone());
        }
    }

    let mut result =
        groups.into_iter().filter(|(_, entries)| !entries.is_empty()).collect::<Vec<_>>();
    if !other.is_empty() {
        result.push(("other", other));
    }
    result
}

pub(crate) fn catalog_count(
    counts: &BTreeMap<String, i64>,
    category: &str,
    rows: &[CatalogRow],
) -> i64 {
    counts.get(category).copied().unwrap_or_else(|| {
        if category == "all" {
            rows.len() as i64
        } else {
            rows.iter().filter(|row| catalog_category_key(&row.category) == category).count() as i64
        }
    })
}

pub(crate) fn catalog_category_key(category: &str) -> &'static str {
    match category {
        "one" | "One-Handed" | "one-handed" => "one",
        "two" | "Two-Handed" | "two-handed" => "two",
        "sta" | "Staff" | "staff" => "sta",
        "shd" | "Shield" | "shield" => "shd",
        "clo" | "Cloth" | "cloth" => "clo",
        "lig" | "Light" | "light" => "lig",
        "hea" | "Heavy" | "heavy" => "hea",
        _ => "other",
    }
}

pub(crate) fn catalog_category_code(category: &str) -> &'static str {
    CATALOG_CATEGORIES
        .iter()
        .find_map(|(key, code)| (*key == category).then_some(*code))
        .unwrap_or("Other")
}

pub(crate) fn catalog_category_label(category: &str) -> String {
    match category {
        "one" => t!("category-one-handed"),
        "two" => t!("category-two-handed"),
        "sta" => t!("category-staff"),
        "shd" => t!("category-shield"),
        "clo" => t!("category-cloth"),
        "lig" => t!("category-light"),
        "hea" => t!("category-heavy"),
        _ => t!("category-other"),
    }
}

pub(crate) fn lot_status_label(status: &str) -> String {
    match status {
        "pending" => t!("status-pending"),
        "live" => t!("status-live"),
        "sold" => t!("status-sold"),
        "unsold" => t!("status-unsold"),
        _ => status.to_owned(),
    }
}

pub(crate) fn lot_status_class(status: &str) -> &'static str {
    match status {
        "pending" => "pending",
        "live" => "live",
        "sold" => "sold",
        "unsold" => "unsold",
        _ => "unknown",
    }
}

pub(crate) fn auction_status_label(status: &str) -> String {
    match status {
        "draft" => t!("auction-status-draft"),
        "live" => t!("auction-status-live"),
        "preview" => t!("auction-status-preview"),
        "final" => t!("auction-status-final"),
        "settling" => t!("auction-status-settling"),
        "cancelled" => t!("auction-status-cancelled"),
        "sold" | "settled" => t!("status-ended"),
        _ => status.to_owned(),
    }
}

pub(crate) fn auction_status_class(status: &str) -> &'static str {
    match status {
        "draft" => "draft",
        "live" => "live",
        "preview" => "preview",
        "final" => "final",
        "settling" => "settling",
        "cancelled" => "cancelled",
        "sold" => "sold",
        "settled" => "settled",
        _ => "unknown",
    }
}

pub(crate) fn auction_world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}

pub(crate) fn historical_auction_href(auction_id: &str, world_id: Option<&str>) -> String {
    let mut query = read_public_query();
    query.auction_id = Some(auction_id.to_owned());
    query.world = world_id.and_then(normalize_world_query);
    public_href("/auction", &query)
}

pub(crate) fn historical_history_href(auction_id: &str, world_id: &str) -> String {
    let mut query = read_public_query();
    query.auction_id = Some(auction_id.to_owned());
    query.world = normalize_world_query(world_id);
    query.category = None;
    query.status = None;
    query.page = 1;
    public_href("/auction", &query)
}

pub(crate) fn lot_detail_href(lot_code: &str) -> String {
    public_href(&format!("/lots/{lot_code}"), &read_public_query())
}

pub(crate) fn current_auction_href() -> String {
    public_href("/auction", &read_public_query())
}

pub(crate) fn format_auction_time(value: &str) -> String {
    format_localized_time(value)
}

/// 解析旧版 Public 使用的 credit 缩写；无后缀和 `k` 表示千，`m` 支持最多两位小数。
///
/// 浏览器只用该结果做即时提示，服务端仍会再次校验金额、拍卖状态和竞价资格。
pub(crate) fn parse_credit_amount(input: &str) -> Option<i64> {
    const MAX_SAFE_JSON_INTEGER: i64 = 9_007_199_254_740_991;

    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let (number, suffix) = match input.chars().last() {
        Some(character) if matches!(character, 'k' | 'K' | 'm' | 'M') => {
            (&input[..input.len() - character.len_utf8()], Some(character))
        }
        Some(_) => (input, None),
        None => return None,
    };
    let (whole, fraction) =
        number.split_once('.').map_or((number, None), |(whole, fraction)| (whole, Some(fraction)));
    if whole.is_empty() || !whole.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    if fraction.is_some_and(|value| {
        value.is_empty()
            || value.len() > 2
            || !value.chars().all(|character| character.is_ascii_digit())
    }) {
        return None;
    }
    if suffix.is_some_and(|character| matches!(character, 'k' | 'K')) && fraction.is_some() {
        return None;
    }
    let whole = whole.parse::<i64>().ok()?;
    let amount = match suffix {
        Some('m' | 'M') => {
            let base = whole.checked_mul(1_000_000)?;
            let fraction = fraction
                .and_then(|value| {
                    let scale = if value.len() == 1 { 100_000 } else { 10_000 };
                    value.parse::<i64>().ok()?.checked_mul(scale)
                })
                .unwrap_or(0);
            base.checked_add(fraction)?
        }
        Some('k' | 'K') | None => whole.checked_mul(1_000)?,
        Some(_) => return None,
    };
    (amount > 0 && amount <= MAX_SAFE_JSON_INTEGER).then_some(amount)
}

pub(crate) struct CountdownDisplay {
    pub(crate) label: String,
    pub(crate) urgent: bool,
    pub(crate) expired: bool,
    pub(crate) empty: bool,
}

pub(crate) fn countdown_display(ends_at: &str, now: DateTime<Utc>) -> CountdownDisplay {
    let Some(end) = DateTime::parse_from_rfc3339(ends_at).ok() else {
        return CountdownDisplay {
            label: t!("value-empty"),
            urgent: false,
            expired: false,
            empty: true,
        };
    };
    let remaining_seconds = (end.with_timezone(&Utc) - now).num_seconds();
    if remaining_seconds <= 0 {
        return CountdownDisplay {
            label: t!("auction-countdown-expired"),
            urgent: false,
            expired: true,
            empty: false,
        };
    }
    let days = remaining_seconds / 86_400;
    let hours = (remaining_seconds % 86_400) / 3_600;
    let minutes = (remaining_seconds % 3_600) / 60;
    let seconds = remaining_seconds % 60;
    let clock = format!("{hours:02}:{minutes:02}:{seconds:02}");
    CountdownDisplay {
        label: if days > 0 {
            t!("auction-countdown-days", days: days, clock: clock)
        } else {
            clock
        },
        urgent: remaining_seconds <= 15 * 60,
        expired: false,
        empty: false,
    }
}

pub(crate) fn countdown_class(countdown: &CountdownDisplay) -> &'static str {
    if countdown.empty {
        "public-auction-countdown public-auction-countdown--none"
    } else if countdown.urgent {
        "public-auction-countdown public-auction-countdown--closing"
    } else if countdown.expired {
        "public-auction-countdown public-auction-countdown--expired"
    } else {
        "public-auction-countdown"
    }
}

/// Public 数据加载、错误和空结果共用的状态面板。
///
/// 该组件只呈现调用方已经判定的安全文案，不吞掉 API 错误，也不把错误正文直接展示给
/// 浏览器；真实数据页面仍由各自的 API 读取分支负责。
#[component]
pub(crate) fn Placeholder(title: String, detail: String) -> Element {
    rsx! {
        section {
            class: "public-auction-empty",
            role: "status",
            aria_live: "polite",
            aria_labelledby: "placeholder-title",
            span { class: "public-auction-empty__mark", aria_hidden: "true" }
            div { class: "public-auction-empty__copy",
                h1 { id: "placeholder-title", "{title}" }
                p { "{detail}" }
            }
        }
    }
}
