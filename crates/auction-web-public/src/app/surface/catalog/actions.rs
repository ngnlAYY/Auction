//! Public 竞价与代理竞价操作面板。

use super::*;

#[component]
pub(crate) fn LiveCatalogActions(
    lot_id: String,
    lot_code: String,
    can_bid: bool,
    current_price: i64,
    next_bid: i64,
    lot_status: String,
    allow_slow: bool,
    session_ready: bool,
    block_ordinary_for_active_proxy: bool,
    authenticated: bool,
    leading: bool,
    show_view: bool,
    show_proxy_summary: bool,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut action = use_signal(|| None::<String>);
    let requested_lot_id = lot_id.clone();
    let proxy_state = use_resource(move || {
        let _ = refresh_generation();
        let should_load_proxy = show_proxy_summary || action() == Some("proxy".to_owned());
        let lot_id = requested_lot_id.clone();
        async move {
            if authenticated && should_load_proxy { fetch_my_proxy(lot_id).await } else { Ok(None) }
        }
    });
    let proxy_snapshot = proxy_state.read_unchecked().clone();
    let active_proxy = match proxy_snapshot.as_ref() {
        Some(Ok(Some(proxy))) if proxy.status == "active" => Some(proxy),
        _ => None,
    };
    let proxy_leading =
        active_proxy.and_then(|proxy| proxy.bid.as_ref()).is_some_and(|bid| bid.leading);
    let effective_leading = leading || proxy_leading;
    let ordinary_blocked =
        effective_leading || (block_ordinary_for_active_proxy && active_proxy.is_some());
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
    // 领先由装备高亮表达；禁用按钮仍通过隐藏说明保留可访问的原因。
    let visually_hide_leading_reason = can_bid && effective_leading;
    let action_reason_id = format!("public-auction-action-reason-{lot_id}");
    let mut proxy_success = use_signal(|| None::<Box<ProxyCommandAuthorization>>);
    let action_name = action();
    let bid_lot_id = lot_id.clone();
    let bid_lot_code = lot_code.clone();
    let proxy_lot_id = lot_id.clone();
    let bid_lot_status = lot_status.clone();
    let proxy_lot_status = lot_status.clone();
    let bid_dialog_title = t!("bid-dialog-title", code: lot_code.clone());
    let proxy_dialog_title = t!("proxy-dialog-title", code: lot_code);

    rsx! {
        span { class: "public-auction-catalog__actions-list",
            if show_view {
                Link { to: lot_detail_href(&lot_id), {t!("catalog-view")} }
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
                    if show_proxy_summary && active_proxy.is_some() {
                        {t!("catalog-lower-proxy")}
                    } else {
                        {t!("catalog-proxy")}
                    }
                }
            } else if can_bid && session_ready {
                Link { to: login_href_for_current_location(), {t!("catalog-login-to-bid")} }
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
                    class: if visually_hide_leading_reason {
                        "public-visually-hidden"
                    } else {
                        "public-auction-catalog__leading-reason"
                    },
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
                        lot_code: bid_lot_code,
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
