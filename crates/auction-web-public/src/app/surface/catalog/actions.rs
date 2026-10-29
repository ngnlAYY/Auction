//! Public 竞价与代理竞价操作面板。

use super::command_scope::{CatalogAction, CommandDialogContext, CommandDialogInput};
use auction_contracts::MAX_PUBLIC_BID_AMOUNT;

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
    let user_id = use_context::<commands::PublicCommandIdentity>().0;
    let mut dialog = use_context::<CommandDialogContext>().0;
    let active_action = use_memo(use_reactive((&lot_id,), move |(lot_id,)| {
        dialog.read().as_ref().filter(|input| input.lot_id == lot_id).map(|input| input.action)
    }));
    let requested_lot_id = lot_id.clone();
    let proxy_state = use_resource(use_reactive(
        (&requested_lot_id, &authenticated, &show_proxy_summary),
        move |(lot_id, authenticated, show_proxy_summary)| {
            // 目录里每行都挂载本资源。若常驻订阅全局 refresh_generation，一次刷新会调度
            // 与行数相同的资源任务（100 行 = 100 个任务）。闭合状态下资源恒为 Ok(None)，
            // 跟随刷新不产生任何可见变化；因此只在本行代理视图激活（代理摘要或代理弹窗
            // 打开）时才订阅刷新节拍，闭合时自动退订。API 没有批量代理查询端点，逐个
            // 预取会改变网络行为，故不做目录级一次性加载。
            let proxy_view_active =
                show_proxy_summary || active_action() == Some(CatalogAction::Proxy);
            if proxy_view_active {
                let _ = refresh_generation();
            }
            async move {
                let result = if authenticated && !user_id.is_nil() && proxy_view_active {
                    fetch_my_proxy(&user_id.to_string(), lot_id.clone()).await
                } else {
                    Ok(None)
                };
                (lot_id, result)
            }
        },
    ));
    use_bid_refresh(
        proxy_state,
        authenticated && (show_proxy_summary || active_action() == Some(CatalogAction::Proxy)),
    );
    let proxy_snapshot = (*proxy_state.state().read() == UseResourceState::Ready)
        .then(|| {
            proxy_state
                .read()
                .as_ref()
                .filter(|(requested_lot, _)| requested_lot == &lot_id)
                .map(|(_, result)| result.clone())
        })
        .flatten();
    let active_proxy = match proxy_snapshot.as_ref() {
        Some(Ok(Some(proxy))) if proxy.status == "active" => Some(proxy),
        _ => None,
    };
    let proxy_leading =
        active_proxy.and_then(|proxy| proxy.bid.as_ref()).is_some_and(|bid| bid.leading);
    let effective_leading = leading || proxy_leading;
    let limit_reached = next_bid > MAX_PUBLIC_BID_AMOUNT;
    let ordinary_blocked = limit_reached
        || effective_leading
        || (block_ordinary_for_active_proxy && active_proxy.is_some());
    // 无法新增出价时，仍允许持有者把已有授权降低到有效绑定价格。
    let proxy_action_enabled = active_proxy.map_or(!effective_leading && !limit_reached, |proxy| {
        let lower_bound = if proxy.capabilities.waiting { next_bid } else { current_price };
        proxy.capabilities.can_lower && lower_bound <= MAX_PUBLIC_BID_AMOUNT
    });
    let action_reason = if !can_bid {
        t!("bid-unavailable")
    } else if limit_reached {
        t!("bid-limit-note")
    } else if active_proxy.is_some() && !effective_leading {
        t!("proxy-active")
    } else if effective_leading {
        t!("bid-leading")
    } else if !proxy_action_enabled {
        t!("proxy-cannot-lower")
    } else {
        String::new()
    };
    // 领先由装备高亮表达；超过金额上限时必须显示说明，让无法出价的原因可见。
    let visually_hide_leading_reason = can_bid && effective_leading && !limit_reached;
    let action_reason_id = format!("public-auction-action-reason-{lot_id}");
    let input = CommandDialogInput {
        lot_id: lot_id.clone(),
        lot_code,
        action: CatalogAction::Bid,
        can_bid,
        current_price,
        next_bid,
        lot_status,
        allow_slow,
        ordinary_blocked,
        leading: effective_leading,
        blocked_reason: action_reason.clone(),
        proxy: proxy_snapshot.clone(),
    };
    use_effect(use_reactive((&input,), move |(mut next,)| {
        let current = dialog.peek().clone();
        if let Some(current) = current.filter(|current| current.lot_id == next.lot_id) {
            next.action = current.action;
            if next != current {
                dialog.set(Some(next));
            }
        }
    }));
    let dropped_lot = lot_id.clone();
    use_drop(move || {
        // GET 错误、分页或阶段切换可卸载行，但不能卸载页面级弹窗或冻结命令。
        if let Ok(mut current) = dialog.try_write()
            && let Some(input) = current.as_mut().filter(|input| input.lot_id == dropped_lot)
        {
            input.can_bid = false;
            input.proxy = None;
        }
    });
    let bid_input = input.clone();
    let proxy_input = input;

    rsx! {
        span { class: "public-auction-catalog__actions-list",
            if show_view {
                Link { to: lot_detail_href(&lot_id), {t!("catalog-view")} }
            }
            if authenticated && (can_bid || show_proxy_summary) {
                button {
                    r#type: "button",
                    aria_haspopup: "dialog",
                    aria_expanded: (active_action() == Some(CatalogAction::Bid)).to_string(),
                    disabled: !can_bid || ordinary_blocked,
                    aria_describedby: if !can_bid || ordinary_blocked {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        // 已打开的模态命令不接受另一个入口替换负载或目标。
                        if dialog.peek().is_none() {
                            dialog.set(Some(bid_input.clone()));
                        }
                    },
                    {t!("catalog-bid")}
                }
                button {
                    r#type: "button",
                    aria_haspopup: "dialog",
                    aria_expanded: (active_action() == Some(CatalogAction::Proxy)).to_string(),
                    disabled: !can_bid || !proxy_action_enabled,
                    aria_describedby: if !can_bid || !proxy_action_enabled {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        if dialog.peek().is_none() {
                            let mut input = proxy_input.clone();
                            input.action = CatalogAction::Proxy;
                            dialog.set(Some(input));
                        }
                    },
                    if show_proxy_summary && active_proxy.is_some() {
                        {t!("catalog-lower-proxy")}
                    } else {
                        {t!("catalog-proxy")}
                    }
                }
            } else if can_bid && session_ready && !limit_reached {
                Link {
                    class: "public-auction-catalog__login",
                    to: login_href_for_current_location(),
                    {t!("catalog-login-to-bid")}
                }
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
    }
}
