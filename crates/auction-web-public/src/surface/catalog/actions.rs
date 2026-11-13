//! Public 竞价与代理竞价操作面板。

use super::command_scope::{CatalogAction, CommandDialogContext, CommandDialogInput};
use super::detail::{CatalogDetailBid, CatalogDetailBidSlot, CatalogDetailDialogContext};
use auction_contracts::MAX_PUBLIC_BID_AMOUNT;

use super::*;

#[component]
pub(crate) fn LiveCatalogActions(
    lot_id: String,
    lot_code: String,
    /// 只用于命令弹窗的拍品摘要，不进入任何命令负载。
    lot_name: String,
    can_bid: bool,
    current_price: i64,
    next_bid: i64,
    lot_status: String,
    allow_slow: bool,
    session_ready: bool,
    block_ordinary_for_active_proxy: bool,
    authenticated: bool,
    leading: bool,
    /// 目录行用紧凑图标入口；拍品详情页用文字按钮。
    icon_only: bool,
    show_proxy_summary: bool,
    /// 目录行的详情抽屉出价入口投影。本组件是出价资格判定的唯一 owner：行内按钮与抽屉
    /// 按钮读取同一份结果，卸载时失败关闭。拍品详情页不传。
    #[props(default)]
    detail_bid: Option<CatalogDetailBidSlot>,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let user_id = use_context::<commands::PublicCommandIdentity>().0;
    let mut dialog = use_context::<CommandDialogContext>().0;
    let active_action = use_memo(use_reactive((&lot_id,), move |(lot_id,)| {
        dialog.read().as_ref().filter(|input| input.lot_id == lot_id).map(|input| input.action)
    }));
    // 详情抽屉打开本拍品时同样激活代理视图：抽屉要展示本人授权的上限与已用次数。只有接了
    // 抽屉的目录行跟踪它；memo 只在“是否打开本拍品”变化时通知，抽屉快照随价格刷新不会重读。
    let detail_dialog = try_use_context::<CatalogDetailDialogContext>().map(|context| context.0);
    let tracks_detail = detail_bid.is_some();
    let detail_open =
        use_memo(use_reactive((&lot_id, &tracks_detail), move |(lot_id, tracks_detail)| {
            tracks_detail
                && detail_dialog.is_some_and(|detail| {
                    detail.read().as_ref().is_some_and(|input| input.row.lot_id == lot_id)
                })
        }));
    let requested_lot_id = lot_id.clone();
    let proxy_state = use_resource(use_reactive(
        (&requested_lot_id, &authenticated, &show_proxy_summary),
        move |(lot_id, authenticated, show_proxy_summary)| {
            // 目录里每行都挂载本资源。若常驻订阅全局 refresh_generation，一次刷新会调度
            // 与行数相同的资源任务（100 行 = 100 个任务）。闭合状态下资源恒为 Ok(None)，
            // 跟随刷新不产生任何可见变化；因此只在本行代理视图激活（代理摘要、代理弹窗或
            // 本拍品的详情抽屉打开）时才订阅刷新节拍，闭合时自动退订。API 没有批量代理查询
            // 端点，逐个预取会改变网络行为，故不做目录级一次性加载。
            let proxy_view_active = show_proxy_summary
                || active_action() == Some(CatalogAction::Proxy)
                || detail_open();
            if proxy_view_active {
                let _ = refresh_generation();
            }
            async move {
                // fetched 区分“真实读取的授权”与闭合状态的占位 Ok(None)。
                let fetched = authenticated && !user_id.is_nil() && proxy_view_active;
                let result = if fetched {
                    fetch_my_proxy(&user_id.to_string(), lot_id.clone()).await
                } else {
                    Ok(None)
                };
                (lot_id, fetched, result)
            }
        },
    ));
    let proxy_view_active = authenticated
        && (show_proxy_summary || active_action() == Some(CatalogAction::Proxy) || detail_open());
    use_bid_refresh(proxy_state, proxy_view_active);
    // 代理视图保持打开期间的刷新（任意拍品的竞价事件都会触发）沿用上一次真实读取的快照，
    // 新结果到达后整体替换：否则表单会被“加载中”顶替而卸载，丢掉已输入的上限、模式与焦点。
    let proxy_ready = *proxy_state.state().read() == UseResourceState::Ready;
    let proxy_snapshot = proxy_state
        .read()
        .as_ref()
        .filter(|(requested_lot, fetched, result)| {
            requested_lot == &lot_id
                && proxy_snapshot_usable(proxy_ready, *fetched, proxy_view_active, result.is_ok())
        })
        .map(|(_, _, result)| result.clone());
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
    // 领先由装备高亮表达，行自身已带“正在领先”的描述，不再重复说明；超过金额上限等
    // 其余原因必须可见。不显示说明时按钮不引用它。
    let show_action_reason = !(can_bid && effective_leading && !limit_reached);
    let action_reason_id = if show_action_reason {
        format!("public-auction-action-reason-{lot_id}")
    } else {
        String::new()
    };
    let input = CommandDialogInput {
        lot_id: lot_id.clone(),
        lot_code,
        lot_name,
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
    // 行内三个分支与抽屉入口共用同一组判定：已登录走命令弹窗，匿名且可出价走登录链接。
    let command_entry = authenticated && (can_bid || show_proxy_summary);
    let login_entry = !command_entry && can_bid && session_ready && !limit_reached;
    let bid_disabled = !can_bid || ordinary_blocked;
    let detail_projection = if command_entry {
        let entry_action = detail_entry_action(bid_disabled, can_bid, effective_leading);
        let mut detail_input = input.clone();
        detail_input.action = entry_action.unwrap_or(CatalogAction::Bid);
        CatalogDetailBid::Command {
            input: Box::new(detail_input),
            disabled: entry_action.is_none(),
        }
    } else if login_entry {
        CatalogDetailBid::Login
    } else {
        CatalogDetailBid::Unavailable
    };
    use_effect(use_reactive((&detail_projection,), move |(next,)| {
        if let Some(CatalogDetailBidSlot(mut detail_bid)) = detail_bid
            && *detail_bid.peek() != next
        {
            detail_bid.set(next);
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
        // 拍品结束后行仍在，但操作栏已卸载：抽屉入口随之失败关闭，不保留最后一次资格。
        if let Some(CatalogDetailBidSlot(mut detail_bid)) = detail_bid
            && let Ok(mut current) = detail_bid.try_write()
        {
            *current = CatalogDetailBid::Unavailable;
        }
    });
    let bid_label = if icon_only {
        t!("catalog-bid-lot", code: input.lot_code.clone())
    } else {
        t!("catalog-bid")
    };
    let bid_input = input.clone();
    let proxy_input = input;

    rsx! {
        span { class: "public-auction-catalog__actions-list",
            // 目录行使用「查看 / 出价 / 标记」三个图标：查看入口属于行（任何阶段都有），
            // 这里只负责出价；代理入口保留在拍品详情页。
            if command_entry {
                button {
                    class: if icon_only { "public-auction-catalog__icon-action" } else { "" },
                    r#type: "button",
                    aria_haspopup: "dialog",
                    aria_expanded: (active_action() == Some(CatalogAction::Bid)).to_string(),
                    disabled: bid_disabled,
                    // 图标按钮的可访问名称带拍品编号，逐行可区分；悬停提示保持简短动作名。
                    aria_label: bid_label,
                    title: icon_only.then(|| t!("catalog-bid")),
                    aria_describedby: if bid_disabled {
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
                    if icon_only {
                        CatalogActionIcon { kind: "gavel" }
                    } else {
                        {t!("catalog-bid")}
                    }
                }
                if show_proxy_summary {
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
                        if active_proxy.is_some() {
                            {t!("catalog-lower-proxy")}
                        } else {
                            {t!("catalog-proxy")}
                        }
                    }
                }
            } else if login_entry {
                Link {
                    class: if icon_only {
                        "public-auction-catalog__icon-action"
                    } else {
                        "public-auction-catalog__login"
                    },
                    to: login_href_for_current_location(),
                    aria_label: t!("catalog-login-to-bid"),
                    title: icon_only.then(|| t!("catalog-login-to-bid")),
                    if icon_only {
                        CatalogActionIcon { kind: "gavel" }
                    } else {
                        {t!("catalog-login-to-bid")}
                    }
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
            if show_action_reason
                && ((show_proxy_summary && !can_bid) || ordinary_blocked || !proxy_action_enabled)
            {
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
    }
}

/// 详情抽屉命令入口打开的页签；`None` 表示入口禁用。
///
/// 抽屉取代了原先进入拍品页的“查看”，是目录里管理代理授权的唯一去处。普通出价可用时进
/// 直接出价（代理由弹窗页签切换）。领先者不能再出价，但可能持有需要降低的代理授权：目录行
/// 不预取授权，无法在这里判断，入口改进代理页签，由弹窗读取后给出降低表单或受阻说明。
fn detail_entry_action(
    ordinary_disabled: bool,
    can_bid: bool,
    leading: bool,
) -> Option<CatalogAction> {
    if !ordinary_disabled {
        Some(CatalogAction::Bid)
    } else if can_bid && leading {
        Some(CatalogAction::Proxy)
    } else {
        None
    }
}

/// 代理授权读取结果能否用于渲染。
///
/// 读取完成（`ready`）的结果总是可用。刷新进行中只沿用上一次真实读取成功的结果，且代理
/// 视图必须仍然打开：闭合状态的占位值与读取失败的结果不沿用，首次读取完成前保持加载，
/// 不把未确认的“没有授权”当成事实，也不让旧错误盖住正在进行的重试。
fn proxy_snapshot_usable(ready: bool, fetched: bool, view_active: bool, succeeded: bool) -> bool {
    ready || (fetched && view_active && succeeded)
}

/// 目录行操作图标；1.5px stroke 与侧栏图标一致，只表达查看、出价语义。
#[component]
pub(super) fn CatalogActionIcon(kind: &'static str) -> Element {
    let path = match kind {
        "eye" => "M1 12s4-7 11-7 11 7 11 7-4 7-11 7S1 12 1 12zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
        "gavel" => "m14 13-7.5 7.5a2.12 2.12 0 0 1-3-3L11 10M16 16l6-6M8 8l6-6M9 7l8 8M21 11l-8-8",
        _ => "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z",
    };
    rsx! {
        svg {
            class: "public-auction-catalog__action-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.5",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: path }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogAction, detail_entry_action, proxy_snapshot_usable};

    #[test]
    fn detail_entry_keeps_proxy_management_reachable_for_leading_bidders() {
        // 普通出价可用：进直接出价，代理由弹窗页签切换。
        assert!(detail_entry_action(false, true, false) == Some(CatalogAction::Bid));
        // 回归：领先者的出价入口被禁用后，仍要能进入代理页签降低已有授权。
        assert!(detail_entry_action(true, true, true) == Some(CatalogAction::Proxy));
        // 达到金额上限的非领先者没有可执行的操作；拍品不可竞价时一律禁用。
        assert!(detail_entry_action(true, true, false).is_none());
        assert!(detail_entry_action(true, false, true).is_none());
        assert!(detail_entry_action(true, false, false).is_none());
    }

    #[test]
    fn finished_read_is_always_usable() {
        // 闭合状态的占位值在读取完成后同样可用：此时没有代理视图依赖它的内容。
        assert!(proxy_snapshot_usable(true, false, false, true));
        assert!(proxy_snapshot_usable(true, true, true, true));
        // 读取失败也要交给表单，由它展示错误与重试入口。
        assert!(proxy_snapshot_usable(true, true, true, false));
    }

    #[test]
    fn refresh_keeps_last_fetched_snapshot_while_view_is_open() {
        // 回归：竞价事件触发的刷新不能卸载正在填写的代理表单。
        assert!(proxy_snapshot_usable(false, true, true, true));
    }

    #[test]
    fn first_read_never_reuses_placeholder() {
        // 打开代理视图后的首次读取完成前只有占位值，必须保持加载而不是显示创建表单。
        assert!(!proxy_snapshot_usable(false, false, true, true));
    }

    #[test]
    fn refresh_drops_snapshot_when_view_closed_or_last_read_failed() {
        assert!(!proxy_snapshot_usable(false, true, false, true));
        assert!(!proxy_snapshot_usable(false, true, true, false));
    }
}
