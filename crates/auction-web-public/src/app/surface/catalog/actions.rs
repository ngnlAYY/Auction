//! Public 竞价与代理竞价操作面板。

use auction_web_shared::{Dialog, use_confirm_command};

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
        // 目录里每行都挂载本资源。若常驻订阅全局 refresh_generation，一次刷新会调度
        // 与行数相同的资源任务（100 行 = 100 个任务）。闭合状态下资源恒为 Ok(None)，
        // 跟随刷新不产生任何可见变化；因此只在本行代理视图激活（代理摘要或代理弹窗
        // 打开）时才订阅刷新节拍，闭合时自动退订。API 没有批量代理查询端点，逐个
        // 预取会改变网络行为，故不做目录级一次性加载。
        let proxy_view_active = show_proxy_summary || action() == Some("proxy".to_owned());
        if proxy_view_active {
            let _ = refresh_generation();
        }
        let lot_id = requested_lot_id.clone();
        async move {
            if authenticated && proxy_view_active { fetch_my_proxy(lot_id).await } else { Ok(None) }
        }
    });
    use_bid_refresh(
        proxy_state,
        authenticated && (show_proxy_summary || action() == Some("proxy".to_owned())),
    );
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
    // 普通竞价成功投影：命令成功后弹窗内展示成功面板（参照代理 proxy_success），
    // 不再直接关弹窗导致用户看不到任何反馈。
    let mut bid_success = use_signal(|| None::<commands::BidSuccess>);
    // 两阶段确认命令 hook 提升到弹窗宿主持有（整体 Copy）：弹窗 scope 一旦销毁，内部
    // spawn 的成功回调（刷新计数与成功面板投影）会随之丢失，服务端已接受出价但 UI 无
    // 反馈；因此提交中必须由 Dialog 的 busy 守卫禁止关闭弹窗。创建与降低上限各持一份，
    // 与原来两个表单各自独占状态的行为一致。
    let mut bid_command = commands::BidConfirmCommand(use_confirm_command());
    let mut proxy_create_command = commands::ProxyConfirmCommand(use_confirm_command());
    let mut proxy_lower_command = commands::ProxyConfirmCommand(use_confirm_command());
    let bid_submitting = bid_command.submitting;
    let proxy_submitting =
        move || (proxy_create_command.submitting)() || (proxy_lower_command.submitting)();
    let any_submitting = move || bid_submitting() || proxy_submitting();
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
                    aria_haspopup: "dialog",
                    aria_expanded: (action() == Some("bid".to_owned())).to_string(),
                    disabled: !can_bid || ordinary_blocked,
                    aria_describedby: if !can_bid || ordinary_blocked {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        // 提交中不允许通过再次点击动作按钮收起弹窗（会丢失成功回调）。
                        if any_submitting() {
                            return;
                        }
                        proxy_success.set(None);
                        bid_success.set(None);
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
                    aria_haspopup: "dialog",
                    aria_expanded: (action() == Some("proxy".to_owned())).to_string(),
                    disabled: !can_bid || !proxy_action_enabled,
                    aria_describedby: if !can_bid || !proxy_action_enabled {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        // 提交中不允许通过再次点击动作按钮收起弹窗（会丢失成功回调）。
                        if any_submitting() {
                            return;
                        }
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
            // 弹窗外壳统一走 shared Dialog：busy 期间 Escape/遮罩/关闭按钮全部禁用，
            // 焦点循环与背景滚动锁由 PublicShell 的 use_dialog_focus_trap(true) 处理。
            Dialog {
                title: bid_dialog_title,
                close_label: t!("catalog-close"),
                busy: bid_submitting(),
                on_close: move |_| {
                    bid_command.reset();
                    bid_success.set(None);
                    action.set(None);
                },
                {commands::bid_form(commands::BidFormInput {
                    lot_id: bid_lot_id,
                    lot_code: bid_lot_code,
                    current_price,
                    next_bid,
                    lot_status: bid_lot_status,
                    can_bid,
                    blocked: ordinary_blocked,
                    blocked_reason: action_reason.clone(),
                    command: bid_command,
                    success: bid_success,
                    on_close: EventHandler::new(move |_: ()| {
                        bid_command.reset();
                        bid_success.set(None);
                        action.set(None);
                    }),
                })}
            }
        }
        if action_name == Some("proxy".to_owned()) {
            Dialog {
                title: proxy_dialog_title,
                close_label: t!("catalog-close"),
                busy: proxy_submitting(),
                on_close: move |_| {
                    proxy_create_command.reset();
                    proxy_lower_command.reset();
                    proxy_success.set(None);
                    action.set(None);
                },
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
                    create_command: proxy_create_command,
                    lower_command: proxy_lower_command,
                    on_close: EventHandler::new(move |_: ()| {
                        proxy_create_command.reset();
                        proxy_lower_command.reset();
                        proxy_success.set(None);
                        action.set(None);
                    }),
                })}
            }
        }
    }
}
