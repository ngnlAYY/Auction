//! 页面级竞价弹窗宿主；读取分支与目录阶段只更新能力，不拥有已提交命令。

use super::detail::{CatalogDetailDialogContext, CatalogDetailInput};
use super::toast::{CatalogToastContext, CatalogToastHost, use_catalog_toast_provider};
use crate::api::ProxyCommandAuthorization;
use crate::surface::{PublicCommandOwner, PublicSessionResource, commands, watches};
use auction_web_shared::{Dialog, use_confirm_command};
use auction_web_shared::{format_credit_amount, t};
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CatalogAction {
    Bid,
    Proxy,
}

/// 这里只保存弹窗展示输入；UUID、冻结负载和未知结果仍由共享 ConfirmCommand 唯一管理。
#[derive(Clone, PartialEq)]
pub(super) struct CommandDialogInput {
    pub lot_id: String,
    pub lot_code: String,
    /// 装备名只用于弹窗摘要，不进入命令负载。
    pub lot_name: String,
    pub action: CatalogAction,
    pub can_bid: bool,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub allow_slow: bool,
    pub ordinary_blocked: bool,
    pub leading: bool,
    pub blocked_reason: String,
    pub proxy: Option<Result<Option<crate::api::OwnedProxy>, String>>,
}

#[derive(Clone, Copy)]
pub(super) struct CommandDialogContext(pub Signal<Option<CommandDialogInput>>);

/// 身份由外壳明确确认；暂时 GET 错误或 Pending 不改变宿主身份。
#[component]
pub(crate) fn CatalogCommandScope(children: Element) -> Element {
    let owner = use_context::<PublicCommandOwner>().0;
    let user_id = owner();
    // 单元素 keyed 列表建立真正的身份生命周期；静态子组件的 key 不会替换 scope。
    rsx! {
        for (user_id, children) in [(user_id, children)] {
            IdentityCommandScope { key: "{user_id:?}", user_id, children }
        }
    }
}

#[component]
fn IdentityCommandScope(user_id: Option<String>, children: Element) -> Element {
    let dialog = use_signal(|| None::<CommandDialogInput>);
    use_context_provider(|| CommandDialogContext(dialog));
    // 详情抽屉的展示信号与命令弹窗同属身份宿主：身份切换时两者一起清空。抽屉本身不产生
    // 任何服务端命令，并由目录内的 CatalogDetailHost 渲染（它需要标记作用域）。
    let detail = use_signal(|| None::<CatalogDetailInput>);
    use_context_provider(|| CatalogDetailDialogContext(detail));
    // 成功提示条比发出它的命令弹窗活得久，因此由身份宿主持有并常驻渲染。
    let toast = use_catalog_toast_provider();
    let pending_watches = watches::use_watch_command_owner(user_id.clone());
    let user_id = user_id
        .as_deref()
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .unwrap_or_else(uuid::Uuid::nil);
    use_context_provider(|| commands::PublicCommandIdentity(user_id));
    let input = dialog.read().clone();
    rsx! {
        {children}
        {pending_watches}
        if let Some(input) = input {
            // 一个页面只有一个模态弹窗；关闭才卸载，后台 GET/分页/阶段切换均不替换它。
            // 它排在 children（含详情抽屉）之后：从抽屉里出价时弹窗是 DOM 顺序上的顶层模态层。
            CatalogCommandDialog { input, dialog }
        }
        CatalogToastHost { toast }
    }
}

#[component]
fn CatalogCommandDialog(
    input: CommandDialogInput,
    mut dialog: Signal<Option<CommandDialogInput>>,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let session = use_context::<PublicSessionResource>();
    let user_id = use_context::<commands::PublicCommandIdentity>().0;
    let authenticated = *session.state().read() == UseResourceState::Ready
        && session.read().as_ref().is_some_and(|snapshot| {
            matches!(snapshot, Ok(Some(identity)) if identity.authenticated
                && uuid::Uuid::parse_str(&identity.user_id).ok() == Some(user_id))
        });
    let toasts = use_context::<CatalogToastContext>();
    // 代理成功投影与命令状态都由稳定宿主持有；表单替换不能取消任务或丢失结果。
    let proxy_success = use_signal(|| None::<Box<ProxyCommandAuthorization>>);
    let mut bid_command = commands::BidConfirmCommand(use_confirm_command());
    let mut proxy_create_command = commands::ProxyConfirmCommand(use_confirm_command());
    let mut proxy_lower_command = commands::ProxyConfirmCommand(use_confirm_command());
    let busy = (bid_command.submitting)()
        || (proxy_create_command.submitting)()
        || (proxy_lower_command.submitting)();
    // 二次确认是方案一确认视图的唯一状态来源；不复制到本地信号，避免命令被替换后
    // 仍显示旧确认行。结果未知时同样隐藏切换条，恢复入口不能被换成另一类命令。
    let confirming = (bid_command.confirmation)().is_some()
        || (proxy_create_command.confirmation)().is_some()
        || (proxy_lower_command.confirmation)().is_some();
    let recovering = (bid_command.uncertain)()
        || (proxy_create_command.uncertain)()
        || (proxy_lower_command.uncertain)();
    let close = EventHandler::new(move |_: ()| {
        if (bid_command.submitting)()
            || (proxy_create_command.submitting)()
            || (proxy_lower_command.submitting)()
        {
            return;
        }
        if (bid_command.confirmation)().is_some()
            || (proxy_create_command.confirmation)().is_some()
            || (proxy_lower_command.confirmation)().is_some()
        {
            // 方案一：确认弹窗的关闭/Escape 先退回编辑态，而不是结束整条命令。
            bid_command.cancel_confirmation();
            proxy_create_command.cancel_confirmation();
            proxy_lower_command.cancel_confirmation();
            return;
        }
        // 显式关闭沿用原有取消契约；自动重读绝不走这个路径。
        dialog.set(None);
    });
    // 方案一：普通出价被接受后直接收起弹窗，结果改由页面右下角的提示条回显。此时命令状态
    // 已由 ConfirmCommand 清空（成功即终态），关闭不会丢弃冻结的 command_id 或未知结果；
    // 出价事实来自服务端响应，提示条只回显它，目录行随后由权威投影刷新。
    let success_code = input.lot_code.clone();
    let mut success_dialog = dialog;
    let bid_succeeded = EventHandler::new(move |success: commands::BidSuccess| {
        let price = format_credit_amount(success.current_price);
        let (message, detail) = if success.leading {
            (t!("bid-success-leading", price: price), None)
        } else {
            // 出价被接受但随即被他人的代理出价超过：同时给出当前价，避免被读成“已领先”。
            (t!("bid-success"), Some(t!("bid-current-price", price: price)))
        };
        toasts.show(success_code.clone(), message, detail);
        success_dialog.set(None);
    });
    // 切换 tab 走同一弹窗快照：actions.rs 的代理资源与刷新订阅据此激活，不另建状态。
    let mut tab_dialog = dialog;
    let switch_tab = EventHandler::new(move |action: CatalogAction| {
        let Some(mut next) = tab_dialog.peek().clone() else { return };
        if next.action == action {
            return;
        }
        next.action = action;
        tab_dialog.set(Some(next));
    });
    let title = if confirming {
        t!("bid-confirm-title")
    } else {
        match input.action {
            CatalogAction::Bid => t!("bid-dialog-title", code: input.lot_code.clone()),
            CatalogAction::Proxy => t!("proxy-dialog-title", code: input.lot_code.clone()),
        }
    };
    let can_bid = input.can_bid && authenticated;
    let lot_name = input.lot_name.clone();
    let lot_code = input.lot_code.clone();
    let current_price = format_credit_amount(input.current_price);
    let next_bid = if input.next_bid > auction_contracts::MAX_PUBLIC_BID_AMOUNT {
        t!("bid-limit-reached")
    } else {
        format_credit_amount(input.next_bid)
    };

    rsx! {
        // 确认视图会把焦点放到“确认”按钮上：按住 Enter 提交金额时，浏览器的重复按键不能
        // 顺势落在确认按钮上替用户完成二次确认。包裹层不参与布局，只吞掉重复的 Enter。
        div {
            class: "public-bid-dialog__keys",
            onkeydown: move |event| {
                if event.key() == Key::Enter && event.is_auto_repeating() {
                    event.prevent_default();
                }
            },
            // 遮罩不关闭；busy 禁用 Escape/关闭，焦点管理仍由 PublicShell 统一提供。
            Dialog {
                title,
                close_label: t!("catalog-close"),
                busy,
                on_close: close,
                if confirming {
                    // 方案一确认弹窗：紧凑拍品行代替摘要块与切换条；类型、金额与约束说明
                    // 由当前表单的确认面板给出。
                    div { class: "public-bid-confirm__lot",
                        span { class: "public-bid-confirm__lot-code", "{lot_code}" }
                        b { class: "public-bid-confirm__lot-title", "{lot_name}" }
                    }
                } else {
                    // 方案一的拍品摘要块：编号、装备名与两个价格在表单之前一次可见。
                    div { class: "public-bid-dialog__lot",
                        div { class: "public-bid-dialog__lot-name",
                            span { class: "public-bid-dialog__lot-code", "{lot_code}" }
                            span { class: "public-bid-dialog__lot-title", "{lot_name}" }
                        }
                        div { class: "public-bid-dialog__lot-facts",
                            div {
                                span { {t!("catalog-current")} }
                                b { "{current_price}" }
                            }
                            div {
                                span { {t!("catalog-next-bid")} }
                                b { "{next_bid}" }
                            }
                        }
                    }
                    if !recovering {
                        // 直接出价/代理出价共用同一弹窗；切换只替换表单内容，不新建命令状态。
                        div { class: "public-bid-dialog__tabs", role: "group", aria_label: t!("bid-type-label"),
                            button {
                                class: if input.action == CatalogAction::Bid {
                                    "public-bid-dialog__tab public-bid-dialog__tab--active"
                                } else {
                                    "public-bid-dialog__tab"
                                },
                                r#type: "button",
                                aria_pressed: (input.action == CatalogAction::Bid).to_string(),
                                disabled: busy,
                                onclick: move |_| switch_tab.call(CatalogAction::Bid),
                                {t!("bid-tab-direct")}
                            }
                            button {
                                class: if input.action == CatalogAction::Proxy {
                                    "public-bid-dialog__tab public-bid-dialog__tab--active"
                                } else {
                                    "public-bid-dialog__tab"
                                },
                                r#type: "button",
                                aria_pressed: (input.action == CatalogAction::Proxy).to_string(),
                                disabled: busy,
                                onclick: move |_| switch_tab.call(CatalogAction::Proxy),
                                {t!("bid-tab-proxy")}
                            }
                        }
                    }
                }
                if input.action == CatalogAction::Bid {
                    if (bid_command.uncertain)() {
                        commands::BidRetry {
                            lot_id: input.lot_id,
                            lot_code: input.lot_code,
                            command: bid_command,
                            on_success: bid_succeeded,
                            on_close: close,
                        }
                    } else {
                        commands::BidForm { input: commands::BidFormInput {
                            lot_id: input.lot_id,
                            next_bid: input.next_bid,
                            lot_status: input.lot_status,
                            can_bid,
                            blocked: input.ordinary_blocked,
                            blocked_reason: input.blocked_reason,
                            command: bid_command,
                            on_success: bid_succeeded,
                            on_close: close,
                        } }
                    }
                } else {
                    {commands::proxy_form(commands::ProxyFormInput {
                        lot_id: input.lot_id,
                        can_bid,
                        current_price: input.current_price,
                        next_bid: input.next_bid,
                        lot_status: input.lot_status,
                        allow_slow: input.allow_slow,
                        leading: input.leading,
                        blocked_reason: input.blocked_reason,
                        proxy: input.proxy,
                        refresh_generation,
                        saved_authorization: proxy_success,
                        create_command: proxy_create_command,
                        lower_command: proxy_lower_command,
                        on_close: close,
                    })}
                }
            }
        }
    }
}
