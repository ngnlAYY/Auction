//! 页面级竞价弹窗宿主；读取分支与目录阶段只更新能力，不拥有已提交命令。

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
use std::rc::Rc;

use auction_web_shared::{Dialog, use_confirm_command};
use auction_web_shared::{format_credit_amount, t};
use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

use super::actions::active_own_proxy;
use super::detail::{CatalogDetailDialogContext, CatalogDetailInput};
use super::toast::{CatalogToastContext, CatalogToastHost, use_catalog_toast_provider};
use crate::api::CatalogRow;
use crate::surface::{AuctionParameter, PublicCommandOwner, commands, read_public_query, watches};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CatalogAction {
    Bid,
    Proxy,
}

/// 表单展示快照；已发出的请求目标与负载归外壳身份宿主。
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
    /// 该场冻结策略的代理默认模式；往期目录拼装的已结算场次为 `None`。
    pub proxy_default_mode: Option<String>,
    pub ordinary_blocked: bool,
    pub leading: bool,
    pub blocked_reason: String,
    pub proxy: Option<Option<crate::api::OwnedProxy>>,
    pub proxy_read: Option<commands::ProxyAuthorizationRead>,
}

impl CommandDialogInput {
    pub(super) fn recovery(row: &CatalogRow) -> Self {
        Self {
            lot_id: row.lot_id.clone(),
            lot_code: row.code.clone(),
            lot_name: row.name.clone(),
            action: CatalogAction::Bid,
            can_bid: false,
            current_price: row.current_price,
            next_bid: row.next_bid,
            lot_status: row.status.clone(),
            allow_slow: false,
            proxy_default_mode: None,
            ordinary_blocked: true,
            leading: false,
            blocked_reason: String::new(),
            proxy: None,
            proxy_read: None,
        }
    }
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
    let mut dialog = use_signal(|| None::<CommandDialogInput>);
    use_context_provider(|| CommandDialogContext(dialog));
    // 详情抽屉的展示信号与命令弹窗同属身份宿主：身份切换时两者一起清空。抽屉本身不产生
    // 任何服务端命令，并由目录内的 CatalogDetailHost 渲染（它需要标记作用域）。
    let detail = use_signal(|| None::<CatalogDetailInput>);
    use_context_provider(|| CatalogDetailDialogContext(detail));
    // 成功提示条比发出它的命令弹窗活得久，因此由身份宿主持有并常驻渲染。
    let toast = use_catalog_toast_provider();
    commands::use_command_session_provider(user_id.clone());
    commands::use_command_auction_provider();
    let pending_watches = watches::use_watch_command_owner(user_id.clone());
    let user_id = user_id
        .as_deref()
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .unwrap_or_else(uuid::Uuid::nil);
    use_context_provider(|| commands::PublicCommandIdentity(user_id));
    let router = dioxus::router::hooks::try_router();
    let invalid = use_memo(move || {
        if let Some(router) = router {
            let _ = router.full_route_string();
        }
        auction_link_invalid()
    });
    let command_auction = use_context::<commands::CommandAuctionContext>().0;
    // 旧会话投影只携带合法 ID。非法地址不能沿该投影回退到当前可出价场次。
    let allowed_auction = use_memo(move || if invalid() { None } else { command_auction() });
    use_context_provider(|| commands::CommandAuctionContext(allowed_auction));
    use_effect(move || {
        if invalid() && dialog.read().is_some() {
            // 作废尚未提交的确认。已发出的命令由外壳保存，沿用原 command_id 和恢复入口。
            dialog.set(None);
        }
    });
    let input = dialog.read().clone();
    rsx! {
        {children}
        {pending_watches}
        if !invalid() && let Some(input) = input {
            // 一个页面只有一个模态弹窗；关闭才卸载，后台 GET/分页/阶段切换均不替换它。
            // 它排在 children（含详情抽屉）之后：从抽屉里出价时弹窗是 DOM 顺序上的顶层模态层。
            CatalogCommandDialog { input, dialog }
        }
        CatalogToastHost { toast }
    }
}

fn auction_link_invalid() -> bool {
    matches!(read_public_query().auction_parameter(), AuctionParameter::Invalid(_))
}

fn use_auction_submit_guard() -> EventHandler<MountedEvent> {
    #[cfg(target_arch = "wasm32")]
    let guard = use_hook(|| Rc::new(RefCell::new(None::<AuctionSubmitGuard>)));
    use_callback(move |_event: MountedEvent| {
        #[cfg(target_arch = "wasm32")]
        if let Some(element) = _event.data().downcast::<web_sys::Element>() {
            guard.replace(AuctionSubmitGuard::install(element.clone()));
        }
    })
}

#[cfg(target_arch = "wasm32")]
struct AuctionSubmitGuard {
    element: web_sys::Element,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}

#[cfg(target_arch = "wasm32")]
impl AuctionSubmitGuard {
    fn install(element: web_sys::Element) -> Option<Self> {
        // 浏览器已经换址、DOM 尚未更新时，旧表单也不能开始准备新的确认。
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            if auction_link_invalid() {
                event.prevent_default();
                event.stop_propagation();
            }
        });
        element
            .add_event_listener_with_callback_and_bool(
                "submit",
                callback.as_ref().unchecked_ref(),
                true,
            )
            .ok()?;
        Some(Self { element, callback })
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for AuctionSubmitGuard {
    fn drop(&mut self) {
        self.element
            .remove_event_listener_with_callback_and_bool(
                "submit",
                self.callback.as_ref().unchecked_ref(),
                true,
            )
            .ok();
    }
}

#[component]
fn CatalogCommandDialog(
    input: CommandDialogInput,
    mut dialog: Signal<Option<CommandDialogInput>>,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let session = use_context::<commands::CommandSession>();
    let toasts = use_context::<CatalogToastContext>();
    let commands = try_use_context::<commands::CompetitionCommands>();
    let origin = use_signal(|| ());
    let active_auction = use_context::<commands::CommandAuctionContext>().0;
    let opened_auction = use_hook(move || active_auction.peek().clone());
    let guard_submit = use_auction_submit_guard();
    let mut receipt = use_signal(|| None::<commands::CommandReceipt>);
    let mut bid_command = commands::BidConfirmCommand(use_confirm_command());
    let mut proxy_create_command = commands::ProxyConfirmCommand(use_confirm_command());
    let mut proxy_lower_command = commands::ProxyConfirmCommand(use_confirm_command());

    use_drop(move || {
        if let Some(commands) = commands {
            commands.detach(origin);
        }
    });
    use_effect(move || {
        let Some(result) = commands
            .and_then(commands::CompetitionCommands::current)
            .and_then(|store| store.take_dialog_receipt(origin))
        else {
            return;
        };
        if result.accepted() && result.kind == commands::CommandKind::Bid {
            toasts.show(result.target.lot_code.clone(), result.message(), None);
            dialog.set(None);
        } else {
            receipt.set(Some(result));
        }
    });
    let store = commands.and_then(commands::CompetitionCommands::current);
    let pending = store.as_ref().and_then(|store| store.command(&input.lot_id));
    let busy = pending.as_ref().is_some_and(|command| command.status.busy());
    let recovering = pending.is_some();
    let feedback = receipt.read().clone();
    let proxy_saved = feedback.as_ref().is_some_and(commands::CommandReceipt::accepted);
    let confirming = !recovering
        && ((bid_command.confirmation)().is_some()
            || (proxy_create_command.confirmation)().is_some()
            || (proxy_lower_command.confirmation)().is_some());
    // 只允许用打开弹窗时的权威场次。切换场次或目录不可用时不得给旧行换世界或场次。
    let target =
        opened_auction.as_ref().filter(|_| opened_auction == active_auction()).map(|auction| {
            Rc::new(commands::CommandTarget {
                world: auction.world.clone(),
                auction_id: auction.auction_id.clone(),
                lot_id: input.lot_id.clone(),
                lot_code: input.lot_code.clone(),
                lot_name: input.lot_name.clone(),
            })
        });
    let target_ready = target.is_some();
    let submit = use_callback(move |payload| {
        // 原生后退可能先于弹窗重绘，提交时须再读当前地址以阻断旧确认。
        if auction_link_invalid() {
            return;
        }
        if let Some(store) = commands.and_then(commands::CompetitionCommands::current)
            && let Some(target) = target.as_ref()
            && store.submit(Rc::clone(target), payload, origin)
        {
            // 确认态只持有未发送输入。外壳接管后清空本地副本，不能从这几个表单直接重试。
            bid_command.reset();
            proxy_create_command.reset();
            proxy_lower_command.reset();
            receipt.set(None);
        }
    });
    let submit_bid =
        use_callback(move |payload| submit.call(commands::CommandPayload::Bid(payload)));
    let submit_proxy =
        use_callback(move |payload| submit.call(commands::CommandPayload::Proxy(payload)));
    let close_lot = input.lot_id.clone();
    let close = use_callback(move |()| {
        if commands
            .and_then(commands::CompetitionCommands::current)
            .and_then(|store| store.command(&close_lot))
            .is_some_and(|command| command.status.busy())
        {
            return;
        }
        if (bid_command.confirmation)().is_some()
            || (proxy_create_command.confirmation)().is_some()
            || (proxy_lower_command.confirmation)().is_some()
        {
            // 确认态的关闭/Escape 先退回编辑态，已发出的记录只由外壳处理。
            bid_command.cancel_confirmation();
            proxy_create_command.cancel_confirmation();
            proxy_lower_command.cancel_confirmation();
            return;
        }
        dialog.set(None);
    });
    // 切换 tab 走同一弹窗快照：actions.rs 的代理资源与刷新订阅据此激活，不另建状态。
    let mut tab_dialog = dialog;
    let switch_tab = use_callback(move |action: CatalogAction| {
        let Some(mut next) = tab_dialog.peek().clone() else { return };
        if next.action == action
            || commands
                .and_then(commands::CompetitionCommands::current)
                .is_some_and(|store| store.command(&next.lot_id).is_some())
            || (action == CatalogAction::Bid && active_own_proxy(next.proxy.as_ref()).is_some())
        {
            return;
        }
        next.action = action;
        tab_dialog.set(Some(next));
    });
    let title = if recovering {
        t!("bid-dialog-title", code: input.lot_code.clone())
    } else if confirming {
        t!("bid-confirm-title")
    } else {
        match input.action {
            CatalogAction::Bid => t!("bid-dialog-title", code: input.lot_code.clone()),
            CatalogAction::Proxy => t!("proxy-dialog-title", code: input.lot_code.clone()),
        }
    };
    let can_bid = input.can_bid && target_ready && session.identity_confirmed();
    let has_active_proxy = active_own_proxy(input.proxy.as_ref()).is_some();
    let active_proxy_reason_id = (has_active_proxy
        && !recovering
        && !proxy_saved
        && (!confirming || input.action == CatalogAction::Bid))
        .then_some("public-bid-active-proxy");
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
            onmounted: guard_submit,
            onkeydown: move |event| {
                if event.key() == Key::Enter && event.is_auto_repeating() {
                    event.prevent_default();
                }
            },
            // 焦点管理由 PublicShell 统一提供。
            Dialog {
                title,
                close_label: t!("catalog-close"),
                busy,
                on_close: close,
                if confirming {
                    // 确认态：紧凑拍品行代替摘要块与切换条；类型、金额与约束说明
                    // 由当前表单的确认面板给出。
                    div { class: "public-bid-confirm__lot",
                        span { class: "public-bid-confirm__lot-code", "{lot_code}" }
                        b { class: "public-bid-confirm__lot-title", "{lot_name}" }
                    }
                } else if !recovering {
                    // 拍品摘要块：编号、装备名与两个价格在表单之前一次可见。
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
                    if !proxy_saved {
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
                                disabled: busy || has_active_proxy,
                                aria_describedby: active_proxy_reason_id,
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
                if let Some(reason_id) = active_proxy_reason_id {
                    p {
                        id: reason_id,
                        class: "action-feedback action-feedback--notice",
                        role: "note",
                        {t!("proxy-active-exists")}
                    }
                }
                div { role: "status", aria_atomic: "true",
                    if let Some(feedback) = feedback {
                        p {
                            class: if proxy_saved { "action-feedback action-feedback--success" } else { "action-feedback action-feedback--error" },
                            {feedback.message()}
                        }
                    }
                }
                if let Some(command) = pending {
                    commands::CommandRecovery { command, origin: Some(origin) }
                } else if proxy_saved {
                    div { class: "public-auction-bid-form__actions",
                        button { r#type: "button", autofocus: true,
                            onclick: move |_| close.call(()), {t!("catalog-close")}
                        }
                    }
                } else if input.action == CatalogAction::Bid {
                    commands::BidForm { input: commands::BidFormInput {
                        lot_id: input.lot_id,
                        next_bid: input.next_bid,
                        lot_status: input.lot_status,
                        can_bid,
                        blocked: input.ordinary_blocked,
                        blocked_reason: input.blocked_reason,
                        active_proxy_reason_id,
                        command: bid_command,
                        on_submit: submit_bid,
                        on_close: close,
                    } }
                } else {
                    commands::ProxyForm { input: commands::ProxyFormInput {
                        lot_id: input.lot_id,
                        can_bid: input.can_bid && target_ready,
                        current_price: input.current_price,
                        next_bid: input.next_bid,
                        lot_status: input.lot_status,
                        allow_slow: input.allow_slow,
                        default_mode: input.proxy_default_mode,
                        leading: input.leading,
                        blocked_reason: input.blocked_reason,
                        proxy: input.proxy,
                        proxy_read: input.proxy_read,
                        refresh_generation,
                        on_submit: submit_proxy,
                        create_command: proxy_create_command,
                        lower_command: proxy_lower_command,
                        on_close: close,
                    } }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::ready;
    use std::rc::Rc;

    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
    use auction_web_shared::{PageVisibility, QueryWriteMode, use_second_ticker};
    use dioxus::dioxus_core::{AttributeValue, ElementId, Mutation, Mutations};
    use dioxus::history::MemoryHistory;
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::super::detail::deep_link_fixture;
    use super::{CatalogAction, CatalogCommandScope, CommandDialogContext, CommandDialogInput};
    use crate::api::{
        BidLogPage, CatalogRow, CommandError, CurrentAuction, HttpResponseFixture,
        PublicSessionApiError, SessionRefreshReason, SessionRequest, TransportError, login_public,
        logout_public,
    };
    use crate::surface::catalog::actions::tests::{ENDED_RECOVERY, waiting_proxy};
    use crate::surface::catalog::columns::CatalogVisibility;
    use crate::surface::catalog::tables::LiveCatalogRow;
    use crate::surface::commands::form_fixture;
    use crate::surface::commands::session_fixture::{
        self, FAIL_SESSION_CHECK, ResponseGate, Session,
    };
    use crate::surface::commands::test_support::{
        ABANDON_CONFIRMATION, BID_SUBMIT, FAIL_BUILD, RECOVERY, Request, enable_requests, requests,
    };
    use crate::surface::commands::{
        CommandStatus, CompetitionCommandHost, CompetitionCommands, ProxyAuthorizationRead,
        use_competition_commands_provider, use_public_session_checks,
    };
    use crate::surface::router::{PUBLIC_LOCALE_CATALOG, provide_test_public_history};
    use crate::surface::watches::{LotWatchScope, SkipWatchReads};
    use crate::surface::{
        AuctionIndex, BoardCatalogueFailureFixture, CurrentAuctionResource, LotDetailHttpFixture,
        LotLinkRequest, LotViewRequest, PageReadFixture, PublicBidRefresh, PublicCommandOwner,
        PublicContentLoading, PublicDisplayedAuction, PublicPageRealtimeTarget,
        PublicReadinessResource, PublicSessionResource, rendered_role_contains,
    };

    const USER: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
    const OTHER_USER: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000002";
    const AUCTION: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000005";
    const LOT: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000006";
    const OTHER_LOT: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000007";
    const RECEIPT_LOT: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000008";

    #[derive(Clone, Copy)]
    struct Controls {
        session: Signal<Session>,
        refresh: Signal<u64>,
        checked_session: PublicSessionResource,
        check_session: EventHandler<SessionRefreshReason>,
        mutate_session: EventHandler<CookieMutation>,
        competition_owner: Signal<Option<String>>,
        ended: Signal<bool>,
        commands: CompetitionCommands,
        view: LotViewRequest,
        location: LotLinkRequest,
        navigate: EventHandler<String>,
        back: EventHandler<()>,
        pause_destination: Signal<bool>,
    }

    #[derive(Clone, Copy)]
    struct EndedRowFixture(Signal<bool>);

    enum CookieMutation {
        Login,
        Logout(&'static str),
    }

    thread_local! {
        static BOARD_DROPS: Cell<usize> = const { Cell::new(0) };
        static CATALOGUE_FAILURE: Cell<bool> = const { Cell::new(false) };
        static LOT_HTTP_STATUS: Cell<Option<u16>> = const { Cell::new(None) };
        static REJECT_DRAWER_ADDRESS: Cell<bool> = const { Cell::new(false) };
        static LOT_HTTP_READS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static COOKIE_RESULT: RefCell<Option<Result<(), String>>> = const { RefCell::new(None) };
        static ARRIVAL: RefCell<Option<(Option<String>, Option<String>)>> = const { RefCell::new(None) };
        static CONTROLS: Cell<Option<Controls>> = const { Cell::new(None) };
        static DIALOG: Cell<Option<Signal<Option<CommandDialogInput>>>> = const { Cell::new(None) };
        static PROXY_READ: Cell<Option<ProxyAuthorizationRead>> = const { Cell::new(None) };
    }

    #[allow(unreachable_code, reason = "Routable 派生对无误段的错误映射不可达")]
    mod routes {
        use dioxus::prelude::*;

        use super::{Away, FixtureShell, TestBoard};
        use crate::surface::{LotDetail, NotFound};

        #[derive(Clone, Debug, PartialEq, Routable)]
        pub(super) enum TestRoute {
            #[layout(FixtureShell)]
            #[route("/?:world&:auction")]
            TestBoard { world: Option<String>, auction: Option<String> },
            #[route("/away")]
            Away {},
            #[route("/lots/:lot_code")]
            LotDetail { lot_code: String },
            #[route("/:..segments")]
            NotFound { segments: Vec<String> },
        }
    }

    #[component]
    fn CommandFixture() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_PUBLIC_LOCALE,
                PUBLIC_LOCALE_CATALOG.default_ftl,
                &DEFAULT_PUBLIC_LOCALE,
                None,
            )
        })
        .expect("加载公开端英文译文");
        rsx! { Router::<routes::TestRoute> {} }
    }

    #[component]
    fn FixtureShell() -> Element {
        let source = session_fixture::use_session_fixture(USER);
        let commands = use_competition_commands_provider();
        let location = use_context_provider(|| LotLinkRequest(Signal::new(None)));
        let view = use_context_provider(|| LotViewRequest(Signal::new(None)));
        use_context_provider(|| PublicContentLoading(Signal::new(Default::default())));
        use_context_provider(PublicBidRefresh::new);
        use_context_provider(|| SkipWatchReads);
        let ended = use_signal(|| false);
        use_context_provider(|| EndedRowFixture(ended));
        use_context_provider(|| {
            LotDetailHttpFixture(|url, _etag| {
                let status = LOT_HTTP_STATUS.get()?;
                LOT_HTTP_READS.with_borrow_mut(|reads| reads.push(url.to_owned()));
                let body = (status == 200).then(fresh_lot_body);
                Some(Box::pin(ready(Ok(HttpResponseFixture { status, etag: None, body }))))
            })
        });
        use_context_provider(|| {
            PageReadFixture::new(|url: String| {
                assert!(url.starts_with(&format!("/api/v1/auctions/{AUCTION}/bid-log?lot={LOT}")));
                Ok::<_, String>(BidLogPage {
                    bids: Vec::new(),
                    page: Some(1),
                    page_size: 4,
                    total: Some(0),
                    cursor: None,
                    next_cursor: None,
                    has_more: None,
                })
            })
        });
        let checks = use_public_session_checks(commands);
        let session = checks.session;
        use_context_provider(|| session);
        use_context_provider(|| PublicCommandOwner(checks.page_owner));
        let auction: CurrentAuctionResource =
            use_resource(|| async { ("isekai".to_owned(), Ok(Some(current_auction("isekai")))) });
        use_context_provider(|| auction);
        use_context_provider(|| Signal::new("isekai".to_owned()));
        let refresh = use_context_provider(|| Signal::new(0_u64));
        let navigator = use_navigator();
        let navigate = use_callback(move |path: String| {
            navigator.push(path);
        });
        let back = use_callback(move |()| navigator.go_back());
        let pause_destination = use_signal(|| false);
        let mutate_session = use_callback(move |mutation| {
            spawn(async move {
                let result = match mutation {
                    CookieMutation::Login => login_public("fixture".to_owned())
                        .await
                        .map(|_| ())
                        .map_err(PublicSessionApiError::into_read_code),
                    CookieMutation::Logout(user_id) => logout_public(user_id.to_owned()).await,
                };
                COOKIE_RESULT.set(Some(result));
            });
        });
        CONTROLS.set(Some(Controls {
            session: source,
            refresh,
            checked_session: session,
            check_session: checks.check,
            mutate_session,
            competition_owner: checks.competition_owner,
            ended,
            commands,
            view,
            location,
            navigate,
            back,
            pause_destination,
        }));
        rsx! {
            CompetitionCommandHost { owner: checks.competition_owner }
            Outlet::<routes::TestRoute> {}
        }
    }

    fn fresh_lot_body() -> String {
        serde_json::json!({
            "code": "ok", "message": "拍品详情",
            "data": {
                "auction_id": AUCTION, "auction_status": "live",
                "projection_kind": "live", "presentation_mode": "live",
                "bidding_open": true, "opens_at": "2026-11-18T12:00:00Z",
                "preview_revision": 0,
                "lot": {
                    "id": LOT, "auction_id": AUCTION, "lot_number": 7,
                    "status": "live", "start_price": 100_000, "current_price": 200_000,
                    "next_bid": 205_000, "has_leader": true, "leader_name": "New Leader",
                    "seller_name": "Seller", "effective_end_at": "2026-11-19T12:00:00Z",
                    "state_version": 2,
                    "equipment": { "name": "Fresh Shield", "category": "Shield",
                        "equipment_group": "rare", "game_equipment_url": "" }
                }
            }
        })
        .to_string()
    }

    fn current_auction(world: &str) -> CurrentAuction {
        CurrentAuction {
            id: AUCTION.to_owned(),
            world: world.to_owned(),
            number: 2,
            status: "live".to_owned(),
            opens_at: "2026-11-18T12:00:00Z".to_owned(),
            scheduled_ends_at: None,
            presentation_mode: "live".to_owned(),
            bidding_open: true,
            preview_revision: 0,
            proxy_default_mode: Some("fast".to_owned()),
        }
    }

    #[component]
    fn TestBoard(world: Option<String>, auction: Option<String>) -> Element {
        use_drop(|| BOARD_DROPS.set(BOARD_DROPS.get() + 1));
        let view = use_context::<LotViewRequest>();
        let arriving = use_hook(|| view.0.peek().is_some());
        if arriving {
            ARRIVAL.set(Some((world, auction)));
            if (CONTROLS.get().unwrap().pause_destination)() {
                return rsx! {};
            }
            rsx! { DestinationBoard {} }
        } else {
            rsx! { CatalogCommandScope { OpenBidFixture {} } }
        }
    }

    #[component]
    fn DestinationBoard() -> Element {
        use_second_ticker();
        use_context_provider(PageVisibility::new);
        let readiness: PublicReadinessResource = use_resource(|| async { Ok(true) });
        use_context_provider(|| readiness);
        use_context_provider(|| PublicDisplayedAuction(Signal::new(None)));
        use_context_provider(|| PublicPageRealtimeTarget(Signal::new(None)));
        let fail_catalogue = use_hook(|| CATALOGUE_FAILURE.get());
        use_context_provider(|| BoardCatalogueFailureFixture(fail_catalogue));
        let current: CurrentAuctionResource = use_resource(move || async move {
            // 宿主 VDOM 没有浏览器地址栏，查询默认选主世界。路由中的冻结位置由 ARRIVAL 单独验证。
            ("main".to_owned(), Ok(fail_catalogue.then(|| current_auction("main"))))
        });
        use_context_provider(|| current);
        use_context_provider(|| Signal::new("main".to_owned()));
        rsx! { AuctionIndex {} }
    }

    #[component]
    fn Away() -> Element {
        rsx! { p { "另一条路由" } }
    }

    fn input(lot_id: &str, action: CatalogAction) -> CommandDialogInput {
        CommandDialogInput {
            lot_id: lot_id.to_owned(),
            lot_code: "S07".to_owned(),
            lot_name: "Legendary Shield".to_owned(),
            action,
            can_bid: true,
            current_price: 100_000,
            next_bid: 105_000,
            lot_status: "live".to_owned(),
            allow_slow: true,
            proxy_default_mode: Some("fast".to_owned()),
            ordinary_blocked: false,
            leading: false,
            blocked_reason: String::new(),
            proxy: Some(None),
            proxy_read: None,
        }
    }

    #[component]
    fn OpenBidFixture() -> Element {
        let mut dialog = use_context::<CommandDialogContext>().0;
        let ended = use_context::<EndedRowFixture>().0;
        let proxy_read: ProxyAuthorizationRead = use_resource(|| async { None });
        PROXY_READ.set(Some(proxy_read));
        DIALOG.set(Some(dialog));
        use_effect(move || dialog.set(Some(input(LOT, CatalogAction::Bid))));
        rsx! {
            if ended() {
                LotWatchScope { auction_id: AUCTION.to_owned(), EndedCatalogRow {} }
            }
        }
    }

    #[component]
    fn EndedCatalogRow() -> Element {
        let row = CatalogRow {
            lot_id: LOT.to_owned(),
            code: "S07".to_owned(),
            name: "Legendary Shield".to_owned(),
            game_equipment_url: String::new(),
            category: "shield".to_owned(),
            equipment_group: "shield".to_owned(),
            status: "sold".to_owned(),
            seller_name: "seller".to_owned(),
            level: None,
            primary_attribute_initials: None,
            avg_roll: None,
            start_price: 100_000,
            current_price: 105_000,
            next_bid: 110_000,
            leader_name: Some("buyer".to_owned()),
            remaining: String::new(),
        };
        rsx! {
            LiveCatalogRow {
                row,
                visibility: CatalogVisibility::load(),
                show_commands: false,
                can_bid: false,
                allow_slow: false,
                proxy_default_mode: None,
                session_ready: true,
                authenticated: true,
                viewer_player_id: None,
            }
        }
    }

    fn settle(dom: &mut VirtualDom) -> Mutations {
        let mut mutations = Mutations::default();
        for _ in 0..16 {
            dom.process_events();
            dom.render_immediate(&mut mutations);
        }
        mutations
    }

    fn mounted() -> VirtualDom {
        mount_with_address(None)
    }

    fn mount_with_address(history: Option<Rc<MemoryHistory>>) -> VirtualDom {
        enable_requests();
        deep_link_fixture::reset();
        ARRIVAL.set(None);
        ENDED_RECOVERY.set(None);
        BOARD_DROPS.set(0);
        CATALOGUE_FAILURE.set(false);
        session_fixture::reset();
        COOKIE_RESULT.set(None);
        LOT_HTTP_STATUS.set(None);
        REJECT_DRAWER_ADDRESS.set(false);
        LOT_HTTP_READS.set(Vec::new());
        let mut dom = VirtualDom::new(CommandFixture);
        if let Some(history) = history {
            provide_test_public_history::<routes::TestRoute>(&mut dom, history, |href, mode| {
                !(REJECT_DRAWER_ADDRESS.get()
                    && mode == QueryWriteMode::Replace
                    && href.starts_with("/?"))
            });
        }
        dom.rebuild(&mut Mutations::default());
        settle(&mut dom);
        assert!(BID_SUBMIT.get().is_some(), "直接出价表单已挂载");
        dom
    }

    fn submit_bid(dom: &mut VirtualDom) -> Mutations {
        dom.in_runtime(|| BID_SUBMIT.get().expect("直接出价表单仍在").call(()));
        settle(dom);
        dom.in_runtime(|| BID_SUBMIT.get().expect("直接出价确认表单仍在").call(()));
        settle(dom)
    }

    fn submit_bid_for(dom: &mut VirtualDom, lot_id: &str) -> Request {
        let mut dialog = DIALOG.get().unwrap();
        dom.in_runtime(|| dialog.set(Some(input(lot_id, CatalogAction::Bid))));
        settle(dom);
        submit_bid(dom);
        requests().pop().unwrap()
    }

    fn navigate(dom: &mut VirtualDom, path: &str) -> Mutations {
        let controls = CONTROLS.get().unwrap();
        dom.in_runtime(|| controls.navigate.call(path.to_owned()));
        settle(dom)
    }

    fn session_refresh(dom: &mut VirtualDom) -> Mutations {
        let controls = CONTROLS.get().unwrap();
        dom.in_runtime(|| controls.check_session.call(SessionRefreshReason::Refresh));
        settle(dom)
    }

    fn mutate_cookie(dom: &mut VirtualDom, mutation: CookieMutation) -> Mutations {
        let controls = CONTROLS.get().unwrap();
        dom.in_runtime(|| controls.mutate_session.call(mutation));
        settle(dom)
    }

    fn confirmed_owner() -> Option<String> {
        CONTROLS.get().unwrap().competition_owner.peek().clone()
    }

    fn element_with_attribute(mutations: &Mutations, name: &str, expected: &str) -> ElementId {
        mutations
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::SetAttribute {
                    name: attribute,
                    value: AttributeValue::Text(value),
                    id,
                    ..
                } if attribute == &name && value == expected => Some(*id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("没有找到 {name}={expected} 的 DOM 元素"))
    }

    fn recovery_link(mutations: &Mutations, lot_id: &str) -> ElementId {
        element_with_attribute(mutations, "href", &format!("/lots/{lot_id}"))
    }

    fn click_view_lot(dom: &mut VirtualDom, link: ElementId) -> Mutations {
        form_fixture::click_link(dom, link);
        settle(dom)
    }

    fn go_away(dom: &mut VirtualDom) {
        let before = BOARD_DROPS.get();
        navigate(dom, "/away");
        assert_eq!(BOARD_DROPS.get(), before + 1, "路由切换确实卸载了看板");
    }

    fn show_unknown(dom: &mut VirtualDom) -> Mutations {
        requests()
            .last()
            .unwrap()
            .complete(Err(CommandError { code: "network_error".to_owned(), status: 0 }));
        settle(dom)
    }

    fn contains_text(mutations: &Mutations, expected: &str) -> bool {
        mutations.edits.iter().any(|edit| match edit {
            Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. } => {
                value.contains(expected)
            }
            _ => false,
        })
    }

    fn status(dom: &VirtualDom, lot_id: &str) -> Option<CommandStatus> {
        dom.in_runtime(|| {
            CONTROLS
                .get()
                .unwrap()
                .commands
                .current()?
                .command(lot_id)
                .map(|command| command.status)
        })
    }

    fn retry(dom: &mut VirtualDom) -> Mutations {
        let (retry, _) = RECOVERY.get().expect("外壳有恢复入口");
        dom.in_runtime(|| retry.call(()));
        settle(dom)
    }

    fn change_session(dom: &mut VirtualDom, session: Session) -> Mutations {
        let mut source = CONTROLS.get().unwrap().session;
        dom.in_runtime(|| source.set(session));
        settle(dom)
    }

    #[test]
    fn leaving_the_board_does_not_cancel_a_sent_bid() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        let submitted = requests();
        assert_eq!(submitted.len(), 1);
        let original = &submitted[0];
        assert!(original.in_flight());
        assert_eq!(status(&dom, LOT), Some(CommandStatus::Submitting));

        go_away(&mut dom);
        assert!(original.in_flight(), "离开看板后请求仍应在途");
        let shown = show_unknown(&mut dom);
        assert!(contains_text(&shown, "Unknown command results: 1"));
        assert!(contains_text(&shown, "Legendary Shield"));
        assert!(contains_text(&shown, "View lot"));
        assert!(shown.edits.iter().any(|edit| matches!(
            edit,
            Mutation::SetAttribute { name: "href", value: AttributeValue::Text(url), .. }
                if url == &format!("/lots/{LOT}")
        )));
        assert_eq!(original.url, format!("/api/v1/lots/{LOT}/bids"));
        assert_eq!(original.user, USER);
        let shown = retry(&mut dom);
        assert!(contains_text(&shown, "Retrying the original command"));
        assert!(matches!(status(&dom, LOT), Some(CommandStatus::Unknown { retrying: true, .. })));
        let replay = requests().pop().unwrap();
        assert_eq!(replay.url, original.url);
        assert_eq!(replay.user, original.user);
        assert_eq!(replay.body, original.body, "实际序列化正文逐字节相同");

        replay.complete(Ok(()));
        let shown = settle(&mut dom);
        assert!(status(&dom, LOT).is_none(), "成功应移除未结记录");
        assert!(contains_text(&shown, "Bid accepted."));
        assert!(contains_text(&shown, "Unknown command results: 0"));
        assert_eq!((CONTROLS.get().unwrap().refresh)(), 2);
        assert!(!contains_text(&settle(&mut dom), "Bid accepted."));
    }

    #[test]
    fn invalid_auction_blocks_old_and_new_confirmations_while_preserving_recovery() {
        let history = Rc::new(MemoryHistory::default());
        let mut dom = mount_with_address(Some(history));
        let original = submit_bid_for(&mut dom, LOT);
        let mut dialog = DIALOG.get().unwrap();
        dom.in_runtime(|| dialog.set(Some(input(OTHER_LOT, CatalogAction::Bid))));
        settle(&mut dom);
        dom.in_runtime(|| BID_SUBMIT.get().unwrap().call(()));
        settle(&mut dom);

        // 地址已变化，模拟浏览器尚未重绘时送到旧确认按钮的事件。
        dom.in_runtime(|| CONTROLS.get().unwrap().navigate.call("/?auction=".to_owned()));
        dom.in_runtime(|| BID_SUBMIT.get().unwrap().call(()));
        settle(&mut dom);
        assert_eq!(requests().len(), 1);
        assert!(original.in_flight());
        assert!(dialog.peek().is_none());

        BID_SUBMIT.set(None);
        dom.in_runtime(|| dialog.set(Some(input(OTHER_LOT, CatalogAction::Bid))));
        settle(&mut dom);
        assert!(BID_SUBMIT.get().is_none(), "无效地址不挂载新的确认表单");
        assert!(dialog.peek().is_none());
        assert!(contains_text(&show_unknown(&mut dom), "Unknown command results: 1"));
        retry(&mut dom);
        let replay = requests().pop().unwrap();
        assert_eq!(replay.url, original.url);
        assert_eq!(replay.body, original.body);
    }

    #[test]
    fn unknown_lot_locks_both_forms_without_locking_other_lots() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        show_unknown(&mut dom);
        let mut dialog = DIALOG.get().unwrap();
        for action in [CatalogAction::Bid, CatalogAction::Proxy] {
            dom.in_runtime(|| dialog.set(None));
            settle(&mut dom);
            BID_SUBMIT.set(None);
            dom.in_runtime(|| dialog.set(Some(input(LOT, action))));
            let mutations = settle(&mut dom);
            assert!(contains_text(&mutations, "retry the original submission"));
            assert!(!mutations.edits.iter().any(|edit| matches!(
                edit,
                Mutation::NewEventListener { name, .. } if name == "submit"
            )));
            assert!(BID_SUBMIT.get().is_none());
            assert_eq!(requests().len(), 1);
        }
        dom.in_runtime(|| dialog.set(None));
        settle(&mut dom);
        dom.in_runtime(|| dialog.set(Some(input(OTHER_LOT, CatalogAction::Bid))));
        settle(&mut dom);
        submit_bid(&mut dom);
        assert_eq!(status(&dom, OTHER_LOT), Some(CommandStatus::Submitting));
        assert!(matches!(status(&dom, LOT), Some(CommandStatus::Unknown { retrying: false, .. })));
        assert_eq!(requests().len(), 2);
    }

    #[test]
    fn abandon_requires_confirmation_and_does_not_send() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        show_unknown(&mut dom);
        go_away(&mut dom);
        assert!(status(&dom, LOT).is_some());
        let (_, abandon) = RECOVERY.get().unwrap();
        dom.in_runtime(|| abandon.call(()));
        let shown = settle(&mut dom);
        assert!(contains_text(&shown, "may already have taken effect"));
        assert!(status(&dom, LOT).is_some());
        let (cancel, _) = ABANDON_CONFIRMATION.get().unwrap();
        dom.in_runtime(|| cancel.call(()));
        settle(&mut dom);
        assert!(status(&dom, LOT).is_some(), "取消确认须保留记录");

        let (_, abandon) = RECOVERY.get().unwrap();
        dom.in_runtime(|| abandon.call(()));
        settle(&mut dom);
        let (_, confirm) = ABANDON_CONFIRMATION.get().unwrap();
        dom.in_runtime(|| confirm.call(()));
        settle(&mut dom);
        assert!(status(&dom, LOT).is_none());
        assert_eq!(requests().len(), 1);
    }

    #[test]
    fn rechecking_keeps_the_record_and_identity_loss_cancels_the_retry() {
        for next in [Some(OTHER_USER), None] {
            let mut dom = mounted();
            submit_bid(&mut dom);
            show_unknown(&mut dom);
            go_away(&mut dom);
            let shown = change_session(&mut dom, Session::Rechecking);
            assert!(contains_text(&shown, "Retry is paused"));
            assert!(shown.edits.iter().any(|edit| matches!(
                edit,
                Mutation::SetAttribute { name: "disabled", value: AttributeValue::Bool(true), .. }
            )));
            assert!(shown.edits.iter().any(|edit| matches!(
                edit,
                Mutation::SetAttribute { name: "aria-describedby", value: AttributeValue::Text(ids), .. }
                    if ids.split_whitespace().any(|id| id.ends_with("-pause"))
            )));
            retry(&mut dom);
            assert_eq!(requests().len(), 1);
            assert!(matches!(
                status(&dom, LOT),
                Some(CommandStatus::Unknown { retrying: false, .. })
            ));

            change_session(&mut dom, Session::Ready(Some(USER)));
            retry(&mut dom);
            let replay = requests().pop().unwrap();
            assert!(replay.in_flight());
            change_session(&mut dom, Session::Ready(next));
            assert!(status(&dom, LOT).is_none());
            assert!(!replay.in_flight());
            assert_eq!(requests().len(), 2, "身份变化后不能自动转发");
        }
    }

    #[test]
    fn a_known_rejection_clears_the_record_and_reports_the_reason() {
        for retry_original in [false, true] {
            let mut dom = mounted();
            submit_bid(&mut dom);
            go_away(&mut dom);
            if retry_original {
                show_unknown(&mut dom);
                retry(&mut dom);
            }
            requests()
                .pop()
                .unwrap()
                .complete(Err(CommandError { status: 409, code: "bid_too_low".to_owned() }));
            let shown = settle(&mut dom);
            assert!(status(&dom, LOT).is_none());
            assert!(contains_text(&shown, "The price has changed."));
            assert!(!contains_text(&settle(&mut dom), "The price has changed."));
        }
    }

    #[test]
    fn a_bid_accepted_in_its_dialog_closes_it_and_shows_the_toast() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        requests().pop().unwrap().complete(Ok(()));
        let shown = settle(&mut dom);
        assert!(status(&dom, LOT).is_none());
        assert!(contains_text(&shown, "Bid accepted."));
        assert!(DIALOG.get().unwrap().peek().is_none());
    }

    fn assert_proxy_replay(lower: bool) {
        let mut dom = mounted();
        let mut dialog = DIALOG.get().unwrap();
        let mut proxy_input = input(LOT, CatalogAction::Proxy);
        proxy_input.proxy_default_mode = Some("slow".to_owned());
        let snapshot = lower.then(waiting_proxy);
        let mut proxy_read = PROXY_READ.get().unwrap();
        let generation = (CONTROLS.get().unwrap().refresh)();
        proxy_input.proxy = Some(snapshot.clone());
        proxy_input.proxy_read = Some(proxy_read);
        dom.in_runtime(|| {
            proxy_read.set(Some(Some((generation, Ok(snapshot)))));
            dialog.set(Some(proxy_input));
        });
        let opened = settle(&mut dom);
        let form = opened
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "submit" => Some(*id),
                _ => None,
            })
            .expect("代理表单必须注册 submit 事件");
        form_fixture::submit(&dom, form);
        settle(&mut dom);
        assert!(requests().is_empty(), "准备确认期间不发送请求");
        form_fixture::submit(&dom, form);
        settle(&mut dom);
        assert_eq!(requests().len(), 1);
        let original = requests().pop().unwrap();
        let authorization = waiting_proxy();
        let url = format!("/api/v1/lots/{LOT}/proxy-authorizations");
        let body: serde_json::Value = serde_json::from_slice(&original.body).unwrap();
        assert_eq!(body["maximum_amount"], 105_000);
        if lower {
            assert_eq!(original.url, format!("{url}/{}/lower", authorization.authorization_id));
            assert!(body.get("mode").is_none());
        } else {
            assert_eq!(original.url, url);
            assert_eq!(body["mode"], "slow");
        }
        show_unknown(&mut dom);
        go_away(&mut dom);
        assert!(matches!(status(&dom, LOT), Some(CommandStatus::Unknown { .. })));

        retry(&mut dom);
        let replay = requests().pop().unwrap();
        assert_eq!(replay.url, original.url);
        assert_eq!(replay.user, original.user);
        assert_eq!(replay.body, original.body);
        replay.complete(Ok(()));
        let shown = settle(&mut dom);
        assert!(contains_text(&shown, "Proxy authorization updated."));
        assert!(status(&dom, LOT).is_none());
    }

    #[test]
    fn create_proxy_form_replays_the_same_serialized_body_and_path() {
        assert_proxy_replay(false);
    }

    #[test]
    fn lower_proxy_form_replays_the_same_serialized_body_and_path() {
        assert_proxy_replay(true);
    }

    #[test]
    fn pre_send_failure_is_reported_without_pending_record_or_refresh() {
        let mut dom = mounted();
        FAIL_BUILD.set(true);
        let shown = submit_bid(&mut dom);
        assert!(requests().is_empty());
        assert!(status(&dom, LOT).is_none());
        assert_eq!((CONTROLS.get().unwrap().refresh)(), 0);
        assert!(DIALOG.get().unwrap().peek().is_some());
        assert!(contains_text(&shown, "This attempt was not sent."));
    }

    #[test]
    fn recheck_between_confirmation_and_send_reports_that_nothing_was_sent() {
        let mut dom = mounted();
        dom.in_runtime(|| BID_SUBMIT.get().unwrap().call(()));
        settle(&mut dom);
        let mut controls = CONTROLS.get().unwrap();
        dom.in_runtime(|| {
            BID_SUBMIT.get().unwrap().call(());
            controls.session.set(Session::Rechecking);
            controls.check_session.call(SessionRefreshReason::Refresh);
        });
        let shown = settle(&mut dom);
        assert!(requests().is_empty());
        assert!(status(&dom, LOT).is_none());
        assert_eq!((controls.refresh)(), 0);
        assert!(contains_text(&shown, "This attempt was not sent."));
        assert!(contains_text(&shown, "once your session is confirmed"));
        change_session(&mut dom, Session::Ready(Some(USER)));
        submit_bid(&mut dom);
        assert_eq!(requests().len(), 1, "复核完成后可以重新提交");
    }

    #[test]
    fn recheck_before_a_retry_sends_nothing_and_keeps_the_original_record() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        go_away(&mut dom);
        show_unknown(&mut dom);
        let original = status(&dom, LOT);
        let (retry, _) = RECOVERY.get().unwrap();
        let mut controls = CONTROLS.get().unwrap();
        dom.in_runtime(|| {
            retry.call(());
            controls.session.set(Session::Rechecking);
            controls.check_session.call(SessionRefreshReason::Refresh);
        });
        let shown = settle(&mut dom);
        assert_eq!(requests().len(), 1);
        assert_eq!(status(&dom, LOT), original);
        assert_eq!((controls.refresh)(), 1);
        assert!(contains_text(&shown, "Retry is paused"));
    }

    #[test]
    fn a_first_command_conflict_is_a_known_rejection() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        go_away(&mut dom);
        requests()
            .pop()
            .unwrap()
            .complete(Err(CommandError { status: 409, code: "command_conflict".to_owned() }));
        let shown = settle(&mut dom);
        assert!(status(&dom, LOT).is_none());
        assert!(contains_text(&shown, "This command conflicts with an earlier request."));
        assert!(!contains_text(&shown, "could not replay"));
        assert_eq!((CONTROLS.get().unwrap().refresh)(), 1);
    }

    #[test]
    fn replay_conflict_keeps_the_record_and_refreshes_each_time() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        let original = requests().pop().unwrap();
        go_away(&mut dom);
        show_unknown(&mut dom);
        for refresh in 2..=3 {
            retry(&mut dom);
            assert!(matches!(
                status(&dom, LOT),
                Some(CommandStatus::Unknown { retrying: true, .. })
            ));
            let replay = requests().pop().unwrap();
            assert_eq!(replay.body, original.body);
            replay.complete(Err(CommandError { status: 409, code: "command_conflict".to_owned() }));
            let shown = settle(&mut dom);
            assert!(matches!(
                status(&dom, LOT),
                Some(CommandStatus::Unknown { retrying: false, .. })
            ));
            assert!(contains_text(&shown, "The server could not replay this command unchanged."));
            assert_eq!((CONTROLS.get().unwrap().refresh)(), refresh);
        }
        let (_, abandon) = RECOVERY.get().unwrap();
        dom.in_runtime(|| abandon.call(()));
        let shown = settle(&mut dom);
        assert!(contains_text(&shown, "Abandon recovery?"));
        let (_, confirm) = ABANDON_CONFIRMATION.get().unwrap();
        dom.in_runtime(|| confirm.call(()));
        settle(&mut dom);
        assert!(status(&dom, LOT).is_none());
        assert_eq!(requests().len(), 3, "放弃记录只改变本地状态");
    }

    #[test]
    fn identity_mismatch_discards_commands_before_the_failed_session_read() {
        for (http_status, code) in [(409, "session_changed"), (401, "unauthenticated")] {
            let mut dom = mounted();
            submit_bid(&mut dom);
            show_unknown(&mut dom);
            let mut dialog = DIALOG.get().unwrap();
            dom.in_runtime(|| dialog.set(Some(input(OTHER_LOT, CatalogAction::Bid))));
            settle(&mut dom);
            submit_bid(&mut dom);
            let other = requests().pop().unwrap();
            assert!(other.in_flight());
            go_away(&mut dom);
            retry(&mut dom);
            let checking = CONTROLS.get().unwrap().checked_session;
            FAIL_SESSION_CHECK.set(true);
            requests()
                .pop()
                .unwrap()
                .complete(Err(CommandError { status: http_status, code: code.to_owned() }));
            let shown = settle(&mut dom);
            assert!(matches!(&*checking.peek(), Some(Err(error)) if error == "network_error"));
            if http_status == 409 {
                assert!(status(&dom, LOT).is_none());
                assert!(status(&dom, OTHER_LOT).is_none());
                assert!(!other.in_flight(), "证实换号立即取消其他任务");
                assert!(contains_text(&shown, "Unknown command results: 0"));
            } else {
                assert!(matches!(
                    status(&dom, LOT),
                    Some(CommandStatus::Unknown { retrying: false, .. })
                ));
                assert!(other.in_flight());
                assert!(contains_text(&shown, "Retry is paused"));
            }
        }
    }

    #[test]
    fn sync_identity_mismatch_discards_commands_even_if_the_following_get_fails() {
        for (http_status, code) in [(409, "session_changed"), (401, "unauthenticated")] {
            let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
            let mut script = vec![
                session_fixture::read(Some(USER)).under_lock(),
                session_fixture::reply(
                    SessionRequest::Sync(USER),
                    session_fixture::rejection(http_status, code),
                )
                .under_lock(),
            ];
            let followup = Rc::new(ResponseGate::default());
            if http_status == 409 {
                script
                    .push(followup.hold(SessionRequest::Read, Err(TransportError::Network(None))));
            }
            session_fixture::script(script);
            let shown = session_refresh(&mut dom);
            if http_status == 409 {
                assert_commands_discarded(&dom, &shown, &other, receipt);
                assert!(followup.waiting(), "销毁后开始复核当前 Cookie");
                followup.release();
                settle(&mut dom);
                assert_session_error("network_error");
                assert!(status(&dom, LOT).is_none());
            } else {
                assert_session_error(code);
                assert!(matches!(
                    status(&dom, LOT),
                    Some(CommandStatus::Unknown { retrying: false, .. })
                ));
                assert!(other.in_flight());
                assert!(!removed(&shown, receipt));
                assert!(contains_text(&shown, "Retry is paused"));
            }
            session_fixture::assert_consumed();
        }
    }

    fn mounted_with_commands_and_receipt() -> (VirtualDom, Request, ElementId) {
        let mut dom = mounted();
        let (other, receipt) = commands_and_receipt(&mut dom);
        (dom, other, receipt)
    }

    fn commands_and_receipt(dom: &mut VirtualDom) -> (Request, ElementId) {
        submit_bid(dom);
        show_unknown(dom);
        let other = submit_bid_for(dom, OTHER_LOT);
        let completed = submit_bid_for(dom, RECEIPT_LOT);
        go_away(dom);
        completed.complete(Ok(()));
        let shown = settle(dom);
        assert!(contains_text(&shown, "Bid accepted."));
        assert!(status(dom, LOT).is_some());
        assert!(other.in_flight());
        let receipt = element_with_attribute(&shown, "aria-label", "Command results");
        (other, receipt)
    }

    fn removed(mutations: &Mutations, element: ElementId) -> bool {
        mutations.edits.iter().any(|edit| {
            matches!(edit, Mutation::Remove { id } | Mutation::ReplaceWith { id, .. } if *id == element)
        })
    }

    fn assert_commands_discarded(
        dom: &VirtualDom,
        shown: &Mutations,
        other: &Request,
        receipt: ElementId,
    ) {
        assert!(status(dom, LOT).is_none(), "旧命令记录已销毁");
        assert!(status(dom, OTHER_LOT).is_none());
        assert!(!other.in_flight(), "旧身份的在途请求已取消");
        assert!(removed(shown, receipt), "旧身份的回执已从 DOM 移除，节点 {receipt:?}");
        assert!(contains_text(shown, "Unknown command results: 0"));
    }

    fn assert_session_error(expected: &str) {
        let session = CONTROLS.get().unwrap().checked_session;
        assert!(matches!(&*session.peek(), Some(Err(error)) if error == expected));
    }

    #[test]
    fn login_establishes_the_new_owner_before_a_failed_followup_get() {
        let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
        let followup = Rc::new(ResponseGate::default());
        session_fixture::script([
            session_fixture::reply(
                SessionRequest::Login("fixture"),
                session_fixture::login_success(OTHER_USER),
            ),
            followup.hold(SessionRequest::Read, Err(TransportError::Network(None))),
        ]);
        let shown = mutate_cookie(&mut dom, CookieMutation::Login);
        assert_eq!(COOKIE_RESULT.take(), Some(Ok(())));
        assert_commands_discarded(&dom, &shown, &other, receipt);
        assert_eq!(confirmed_owner().as_deref(), Some(OTHER_USER));
        assert!(followup.waiting());
        followup.release();
        settle(&mut dom);
        session_fixture::assert_consumed();
        assert_session_error("network_error");
        assert_eq!(confirmed_owner().as_deref(), Some(OTHER_USER));
        assert!(
            dom.in_runtime(|| CONTROLS.get().unwrap().commands.current().is_some()),
            "登录响应已建立 B 的宿主"
        );
    }

    #[test]
    fn late_logout_conflict_only_discards_the_request_expected_owner() {
        let mut dom = mounted();
        let response = Rc::new(ResponseGate::default());
        session_fixture::script([response.hold(
            SessionRequest::Logout(USER),
            session_fixture::rejection(409, "session_changed"),
        )]);
        mutate_cookie(&mut dom, CookieMutation::Logout(USER));
        assert!(response.waiting());
        change_session(&mut dom, Session::Ready(Some(OTHER_USER)));
        assert_eq!(confirmed_owner().as_deref(), Some(OTHER_USER));
        let (other, receipt) = commands_and_receipt(&mut dom);
        assert_eq!(other.user, OTHER_USER);
        let before = status(&dom, LOT);

        session_fixture::script([session_fixture::reply(
            SessionRequest::Read,
            Err(TransportError::Timeout),
        )]);
        response.release();
        let shown = settle(&mut dom);
        session_fixture::assert_consumed();
        assert_session_error("request_timeout");
        assert_eq!(COOKIE_RESULT.take(), Some(Err("session_changed".to_owned())));
        assert_eq!(confirmed_owner().as_deref(), Some(OTHER_USER));
        assert_eq!(status(&dom, LOT), before);
        assert!(other.in_flight());
        assert!(!removed(&shown, receipt));

        session_fixture::script([
            session_fixture::reply(
                SessionRequest::Logout(OTHER_USER),
                session_fixture::rejection(409, "session_changed"),
            ),
            session_fixture::reply(SessionRequest::Read, Err(TransportError::Timeout)),
        ]);
        let shown = mutate_cookie(&mut dom, CookieMutation::Logout(OTHER_USER));
        session_fixture::assert_consumed();
        assert_session_error("request_timeout");
        assert_commands_discarded(&dom, &shown, &other, receipt);
        assert!(confirmed_owner().is_none());
    }

    #[test]
    fn logout_success_publishes_anonymous_before_a_failed_followup_get() {
        let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
        session_fixture::script([
            session_fixture::reply(
                SessionRequest::Logout(USER),
                Ok(session_fixture::response(
                    200,
                    r#"{"code":"ok","message":"已退出","data":null}"#.to_owned(),
                )),
            ),
            session_fixture::reply(SessionRequest::Read, Err(TransportError::Timeout)),
        ]);
        let shown = mutate_cookie(&mut dom, CookieMutation::Logout(USER));
        session_fixture::assert_consumed();
        assert_eq!(COOKIE_RESULT.take(), Some(Ok(())));
        assert_session_error("request_timeout");
        assert_commands_discarded(&dom, &shown, &other, receipt);
        assert!(confirmed_owner().is_none());
    }

    #[test]
    fn locked_get_discards_old_identity_before_cookie_sync_fails() {
        for (failure, code) in [
            (Err(TransportError::Timeout), "request_timeout"),
            (session_fixture::rejection(503, "temporarily_unavailable"), "temporarily_unavailable"),
        ] {
            let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
            let sync = Rc::new(ResponseGate::default());
            session_fixture::script([
                session_fixture::read(Some(OTHER_USER)).under_lock(),
                sync.hold(SessionRequest::Sync(OTHER_USER), failure).under_lock(),
            ]);
            let shown = session_refresh(&mut dom);
            session_fixture::assert_consumed();
            assert!(sync.waiting(), "Cookie 同步请求尚未得到响应");
            assert_commands_discarded(&dom, &shown, &other, receipt);
            assert_eq!(confirmed_owner().as_deref(), Some(OTHER_USER));
            sync.release();
            settle(&mut dom);
            assert_session_error(code);
            assert!(status(&dom, LOT).is_none());
        }
    }

    #[test]
    fn first_locked_get_discards_old_identity_before_a_successful_cookie_sync() {
        let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
        let sync = Rc::new(ResponseGate::default());
        session_fixture::take_requests();
        session_fixture::script([
            session_fixture::read(Some(OTHER_USER)).under_lock(),
            sync.hold(
                SessionRequest::Sync(OTHER_USER),
                session_fixture::session_response_with_deadline(Some(OTHER_USER), None),
            )
            .under_lock(),
        ]);
        let shown = session_refresh(&mut dom);
        session_fixture::assert_consumed();
        assert_eq!(session_fixture::take_requests(), ["GET", "PUT"]);
        assert!(sync.waiting());
        assert_commands_discarded(&dom, &shown, &other, receipt);
        sync.release();
        settle(&mut dom);
        assert_eq!(confirmed_owner().as_deref(), Some(OTHER_USER));
        assert!(matches!(&*CONTROLS.get().unwrap().checked_session.peek(),
            Some(Ok(Some(identity))) if identity.user_id == OTHER_USER));
        assert!(session_fixture::take_requests().is_empty());
        assert!(status(&dom, LOT).is_none());
    }

    #[test]
    fn anonymous_get_discards_old_identity_even_if_the_next_read_fails() {
        let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
        session_fixture::take_requests();
        session_fixture::script([session_fixture::read(None).under_lock()]);
        let shown = session_refresh(&mut dom);
        session_fixture::assert_consumed();
        assert_eq!(session_fixture::take_requests(), ["GET"]);
        assert!(matches!(&*CONTROLS.get().unwrap().checked_session.peek(), Some(Ok(None))));
        assert_commands_discarded(&dom, &shown, &other, receipt);
        session_fixture::script([session_fixture::reply(
            SessionRequest::Read,
            Err(TransportError::Network(None)),
        )]);
        session_refresh(&mut dom);
        session_fixture::assert_consumed();
        assert_session_error("network_error");
        assert!(status(&dom, LOT).is_none());
        assert!(!other.in_flight());
    }

    #[test]
    fn failed_get_does_not_confirm_an_identity_change() {
        for (failure, code) in [
            (Err(TransportError::Network(None)), "network_error"),
            (session_fixture::rejection(401, "unauthenticated"), "unauthenticated"),
        ] {
            let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
            session_fixture::script([session_fixture::reply(SessionRequest::Read, failure)]);
            let shown = session_refresh(&mut dom);
            session_fixture::assert_consumed();
            assert_session_error(code);
            assert!(matches!(
                status(&dom, LOT),
                Some(CommandStatus::Unknown { retrying: false, .. })
            ));
            assert!(other.in_flight());
            assert!(!removed(&shown, receipt));
            assert!(contains_text(&shown, "Retry is paused"));
        }
    }

    #[test]
    fn malformed_success_envelopes_do_not_establish_anonymous_identity() {
        for body in [
            r#"["ok","缺少对象信封",null]"#,
            r#"{"code":"ok","message":"缺少会话投影"}"#,
            r#"{"code":"accepted","message":"错误的成功码","data":null}"#,
        ] {
            let (mut dom, other, receipt) = mounted_with_commands_and_receipt();
            session_fixture::script([session_fixture::reply(
                SessionRequest::Read,
                Ok(session_fixture::response(200, body.to_owned())),
            )]);
            let shown = session_refresh(&mut dom);
            session_fixture::assert_consumed();
            assert_session_error("invalid_response");
            assert_eq!(confirmed_owner().as_deref(), Some(USER));
            assert!(status(&dom, LOT).is_some());
            assert!(other.in_flight());
            assert!(!removed(&shown, receipt));
            assert!(contains_text(&shown, "Retry is paused"));
        }
    }

    #[test]
    fn fallback_redirect_reuses_the_identity_host() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        show_unknown(&mut dom);
        let mut dialog = DIALOG.get().unwrap();
        dom.in_runtime(|| dialog.set(Some(input(OTHER_LOT, CatalogAction::Bid))));
        settle(&mut dom);
        submit_bid(&mut dom);
        let other = requests().pop().unwrap();
        navigate(&mut dom, "/unregistered/path");
        assert_eq!(BOARD_DROPS.get(), 1);
        assert!(DIALOG.get().unwrap().try_peek().is_ok(), "兜底页已回首页");
        assert!(status(&dom, LOT).is_some());
        assert!(other.in_flight());
    }

    #[test]
    fn view_lot_opens_the_frozen_rounds_drawer_after_catalogue_failure() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        let link = recovery_link(&show_unknown(&mut dom), LOT);
        // 在看板上触发查看，离开看板时清理旧抽屉请求不能覆盖本次冻结位置。
        deep_link_fixture::script(LOT, AUCTION, 1);
        CATALOGUE_FAILURE.set(true);
        let failed = click_view_lot(&mut dom, link);
        assert!(contains_text(&failed, "temporarily unavailable"));
        assert_eq!(ARRIVAL.take(), None);
        dom.in_runtime(|| deep_link_fixture::RETRY.get().unwrap().call(()));
        let shown = settle(&mut dom);
        assert_eq!(BOARD_DROPS.get(), 1);
        assert_eq!(ARRIVAL.take(), Some((Some("isekai".to_owned()), Some(AUCTION.to_owned()))));
        assert!(deep_link_fixture::location_lookups().is_empty());
        assert!(
            contains_text(&shown, "Auction data is temporarily unavailable."),
            "真实目录进入最终错误态"
        );
        assert!(contains_text(&shown, "Authority Shield"), "已打开目标拍品抽屉");
        assert!(CONTROLS.get().unwrap().view.0.peek().is_none());
        assert!(!contains_text(&settle(&mut dom), "Authority Shield"), "只消费一次");
        assert!(status(&dom, LOT).is_some());
    }

    #[test]
    fn successful_retry_requires_a_new_lot_get_before_the_read_only_drawer_opens() {
        crate::surface::run_page_events(
            module_path!(),
            "successful_retry_requires_a_new_lot_get_before_the_read_only_drawer_opens",
            exercise_retry_then_drawer,
        );
    }

    #[test]
    fn a_drawer_opened_during_retry_retracts_its_invalidated_projection() {
        crate::surface::run_page_events(
            module_path!(),
            "a_drawer_opened_during_retry_retracts_its_invalidated_projection",
            || {
                let mut dom = mounted();
                submit_bid(&mut dom);
                let link = recovery_link(&show_unknown(&mut dom), LOT);
                retry(&mut dom);
                let replay = requests().pop().unwrap();
                deep_link_fixture::script(LOT, AUCTION, 0);
                CATALOGUE_FAILURE.set(true);
                let waiting = click_view_lot(&mut dom, link);
                assert!(contains_text(&waiting, "Authority Shield"));
                assert!(LOT_HTTP_READS.take().is_empty());
                let drawer = crate::surface::rendered_event_target(
                    &dom,
                    "aside",
                    "class",
                    "public-catalog-detail",
                    "onkeydown",
                );

                LOT_HTTP_STATUS.set(Some(503));
                replay.complete(Ok(()));
                let failed = settle(&mut dom);
                assert_eq!(LOT_HTTP_READS.take(), [format!("/api/v1/lots/{LOT}")]);
                assert!(contains_text(&failed, "Bid accepted."));
                assert!(removed(&failed, drawer), "撤下已失效的详情抽屉");
                let retry = crate::surface::rendered_event_target(
                    &dom,
                    "button",
                    "id",
                    "public-lot-detail-retry",
                    "onclick",
                );
                LOT_HTTP_STATUS.set(Some(200));
                crate::surface::dispatch_page_event(&dom, "click", retry, "");
                let loaded = settle(&mut dom);
                assert_eq!(LOT_HTTP_READS.take(), [format!("/api/v1/lots/{LOT}")]);
                assert!(contains_text(&loaded, "Fresh Shield"));
                assert!(contains_text(&loaded, "New Leader"));
                assert!(contains_text(&loaded, "200K"));
            },
        );
    }

    fn exercise_retry_then_drawer() {
        for status in [200, 404, 410, 503] {
            let mut dom = mounted();
            submit_bid(&mut dom);
            let link = recovery_link(&show_unknown(&mut dom), LOT);
            retry(&mut dom);
            let replay = requests().pop().unwrap();
            let controls = CONTROLS.get().unwrap();
            let mut pause = controls.pause_destination;
            dom.in_runtime(|| pause.set(true));
            deep_link_fixture::script(LOT, AUCTION, 0);
            CATALOGUE_FAILURE.set(true);
            let waiting = click_view_lot(&mut dom, link);
            assert!(!contains_text(&waiting, "Authority Shield"));
            assert!(controls.view.0.peek().is_some(), "详情已预读并交到目标看板");
            replay.complete(Ok(()));
            let receipt = settle(&mut dom);
            assert!(contains_text(&receipt, "Bid accepted."));
            assert_eq!((controls.refresh)(), 2);

            LOT_HTTP_STATUS.set(Some(status));
            dom.in_runtime(|| pause.set(false));
            let shown = settle(&mut dom);
            assert_eq!(LOT_HTTP_READS.take(), [format!("/api/v1/lots/{LOT}")]);
            assert!(!contains_text(&shown, "Authority Shield"));
            match status {
                200 => {
                    assert!(contains_text(&shown, "Fresh Shield"));
                    assert!(contains_text(&shown, "New Leader"));
                    assert!(contains_text(&shown, "200K"));
                }
                404 | 410 => {
                    let message = "This lot does not exist or is no longer available.";
                    assert!(contains_text(&shown, message));
                    assert!(rendered_role_contains(&dom, "status", message));
                    assert!(!rendered_role_contains(&dom, "alert", message));
                }
                503 => {
                    assert!(rendered_role_contains(&dom, "alert", "View lot S07"));
                    assert!(!rendered_role_contains(&dom, "status", "View lot S07"));
                    assert!(contains_text(&shown, "Retry"));
                    let retry = crate::surface::rendered_event_target(
                        &dom,
                        "button",
                        "id",
                        "public-lot-detail-retry",
                        "onclick",
                    );
                    LOT_HTTP_STATUS.set(Some(200));
                    crate::surface::dispatch_page_event(&dom, "click", retry, "");
                    let loaded = settle(&mut dom);
                    assert_eq!(LOT_HTTP_READS.take(), [format!("/api/v1/lots/{LOT}")]);
                    assert!(contains_text(&loaded, "New Leader"));
                }
                _ => unreachable!("用例只提供登记的 HTTP 状态"),
            }
        }
    }

    #[test]
    fn going_back_to_the_same_board_url_drops_the_pending_lot_request() {
        let history = Rc::new(MemoryHistory::default());
        let mut dom = mount_with_address(Some(history.clone()));
        deep_link_fixture::script(LOT, AUCTION, 0);
        let controls = CONTROLS.get().unwrap();
        let mut pause = controls.pause_destination;
        dom.in_runtime(|| pause.set(true));
        navigate(&mut dom, &format!("/lots/{LOT}"));
        let view = controls.view.0;
        assert!(view.peek().as_ref().is_some_and(|target| target.entry.is_some()));
        assert!(history.can_go_back());
        dom.in_runtime(|| controls.back.call(()));
        assert_eq!(history.current_route(), "/");
        // 允许返回后旧看板仍挂载，必须由历史条目门禁丢弃已经交付的请求。
        dom.in_runtime(|| {
            assert!(view.peek().is_some());
            pause.set(false);
        });
        let shown = settle(&mut dom);
        assert!(!contains_text(&shown, "Authority Shield"));
        assert!(view.peek().is_none());
        assert!(LOT_HTTP_READS.take().is_empty());
    }

    #[test]
    fn a_rejected_drawer_destination_keeps_the_link_retryable() {
        let history = Rc::new(MemoryHistory::default());
        let mut dom = mount_with_address(Some(history.clone()));
        deep_link_fixture::script(LOT, AUCTION, 0);
        REJECT_DRAWER_ADDRESS.set(true);
        let source = format!("/lots/{LOT}");
        let failed = navigate(&mut dom, &source);
        assert_eq!(history.current_route(), source);
        assert!(contains_text(&failed, "temporarily unavailable"));
        assert!(CONTROLS.get().unwrap().view.0.peek().is_none());
        REJECT_DRAWER_ADDRESS.set(false);
        dom.in_runtime(|| deep_link_fixture::RETRY.get().unwrap().call(()));
        let shown = settle(&mut dom);
        assert!(history.current_route().starts_with("/?"));
        assert!(contains_text(&shown, "Authority Shield"));
    }

    #[test]
    fn moving_between_lot_links_releases_the_second_lots_frozen_location() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        let link_a = recovery_link(&show_unknown(&mut dom), LOT);
        submit_bid_for(&mut dom, OTHER_LOT);
        let link_b = recovery_link(&show_unknown(&mut dom), OTHER_LOT);

        deep_link_fixture::script(LOT, AUCTION, 2);
        let failed = click_view_lot(&mut dom, link_a);
        assert!(contains_text(&failed, "temporarily unavailable"));
        assert!(deep_link_fixture::location_lookups().is_empty());

        deep_link_fixture::script(OTHER_LOT, AUCTION, 0);
        let opened = click_view_lot(&mut dom, link_b);
        assert!(contains_text(&opened, "Authority Shield"));
        assert_eq!(ARRIVAL.take(), Some((Some("isekai".to_owned()), Some(AUCTION.to_owned()))));
        assert!(deep_link_fixture::location_lookups().is_empty());
        assert!(CONTROLS.get().unwrap().location.0.peek().is_none(), "交付后不再保留 B 的冻结位置");

        let shown = navigate(&mut dom, &format!("/lots/{OTHER_LOT}"));
        assert!(contains_text(&shown, "Authority Shield"));
        assert_eq!(ARRIVAL.take(), Some((None, None)), "普通入口应重新解析场次");
        assert_eq!(deep_link_fixture::location_lookups(), ["main", "isekai", AUCTION]);
    }

    #[test]
    fn ordinary_deep_link_opens_the_drawer_on_the_real_empty_board() {
        let mut dom = mounted();
        deep_link_fixture::script(LOT, AUCTION, 1);
        let failed = navigate(&mut dom, &format!("/lots/{LOT}"));
        assert!(contains_text(&failed, "temporarily unavailable"));
        assert_eq!(ARRIVAL.take(), None);
        dom.in_runtime(|| deep_link_fixture::RETRY.get().unwrap().call(()));
        let shown = settle(&mut dom);
        assert!(contains_text(&shown, "No auction in this world yet"));
        assert!(contains_text(&shown, "Authority Shield"));
        assert!(CONTROLS.get().unwrap().view.0.peek().is_none());
        assert_eq!(ARRIVAL.take(), Some((None, None)));
        assert_eq!(deep_link_fixture::location_lookups().last().map(String::as_str), Some(AUCTION));
    }

    #[test]
    fn http_404_shows_missing_lot_from_an_ordinary_link() {
        assert_http_missing_lot(404, false);
    }

    #[test]
    fn http_404_shows_missing_lot_from_a_recovery_link() {
        assert_http_missing_lot(404, true);
    }

    #[test]
    fn http_410_shows_missing_lot() {
        assert_http_missing_lot(410, false);
    }

    fn assert_http_missing_lot(http_status: u16, from_command: bool) {
        let mut dom = mounted();
        deep_link_fixture::use_wire_response();
        LOT_HTTP_STATUS.set(Some(http_status));
        let shown = if from_command {
            submit_bid(&mut dom);
            let link = recovery_link(&show_unknown(&mut dom), LOT);
            click_view_lot(&mut dom, link)
        } else {
            navigate(&mut dom, &format!("/lots/{LOT}"))
        };
        assert!(contains_text(&shown, "This lot does not exist or is no longer available."));
        assert!(!contains_text(&shown, "temporarily unavailable"));
        assert!(deep_link_fixture::RETRY.get().is_none());
        assert_eq!(LOT_HTTP_READS.take(), [format!("/api/v1/lots/{LOT}")]);
        assert_eq!(ARRIVAL.take(), None);
        assert!(CONTROLS.get().unwrap().view.0.peek().is_none());
        assert_eq!(status(&dom, LOT).is_some(), from_command);
    }

    #[test]
    fn http_503_keeps_the_lot_link_retryable() {
        let mut dom = mounted();
        deep_link_fixture::use_wire_response();
        LOT_HTTP_STATUS.set(Some(503));
        let shown = navigate(&mut dom, &format!("/lots/{LOT}"));
        assert!(contains_text(&shown, "temporarily unavailable"));
        assert!(deep_link_fixture::RETRY.get().is_some());
        assert_eq!(LOT_HTTP_READS.take(), [format!("/api/v1/lots/{LOT}")]);
        assert_eq!(ARRIVAL.take(), None);
    }

    #[test]
    fn ended_lot_row_keeps_its_read_only_recovery_entry() {
        let mut dom = mounted();
        submit_bid(&mut dom);
        show_unknown(&mut dom);
        let mut dialog = DIALOG.get().unwrap();
        let mut ended = CONTROLS.get().unwrap().ended;
        dom.in_runtime(|| {
            dialog.set(None);
            ended.set(true);
        });
        let shown = settle(&mut dom);
        assert!(contains_text(&shown, "This lot has a command with an unknown result."));
        BID_SUBMIT.set(None);
        dom.in_runtime(|| ENDED_RECOVERY.get().expect("已结束行保留恢复入口").call(()));
        let opened = settle(&mut dom);
        assert!(contains_text(&opened, "retry the original submission"));
        assert!(BID_SUBMIT.get().is_none());
        assert_eq!(requests().len(), 1);
    }
}
