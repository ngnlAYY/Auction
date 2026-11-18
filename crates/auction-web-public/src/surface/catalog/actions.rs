//! Public 竞价与代理竞价操作面板。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::t;
use dioxus::prelude::*;
use uuid::Uuid;

use super::command_scope::{CatalogAction, CommandDialogContext, CommandDialogInput};
use super::detail::{CatalogDetailBid, CatalogDetailBidSlot, CatalogDetailDialogContext};
use crate::api::{CatalogRow, OwnedProxy, fetch_my_proxy};
use crate::surface::session::login_href_for_current_location;
use crate::surface::{commands, use_bid_refresh};

type ProxyRead = Result<Option<OwnedProxy>, String>;

#[derive(Clone, Default)]
struct CatalogProxySnapshots(Rc<RefCell<BTreeMap<String, ProxyRead>>>);

/// 由按身份和场次加键的目录提供，让实际读取的结果跨抽屉关闭和目录行卸载保留。
pub(crate) fn use_catalog_proxy_snapshots() {
    use_context_provider(CatalogProxySnapshots::default);
}

/// 结束拍品只恢复该件已发出的命令；不建立出价资格或私有代理读取。
#[component]
pub(super) fn CatalogRecoveryEntry(row: CatalogRow) -> Element {
    let mut dialog = use_context::<CommandDialogContext>().0;
    let commands = try_use_context::<commands::CompetitionCommands>();
    let pending = use_memo(use_reactive((&row.lot_id,), move |(lot_id,)| {
        commands
            .and_then(commands::CompetitionCommands::current)
            .and_then(|store| store.command(&lot_id))
            .map(|command| command.status)
    }));
    let reason_id = format!("public-auction-action-reason-{}", row.lot_id);
    let label = t!("command-recovery-open", code: row.code.clone());
    let open = use_callback(move |()| {
        if dialog.peek().is_none()
            && commands
                .and_then(commands::CompetitionCommands::current)
                .is_some_and(|store| store.command(&row.lot_id).is_some())
        {
            dialog.set(Some(CommandDialogInput::recovery(&row)));
        }
    });
    let Some(status) = pending() else { return rsx! {} };
    #[cfg(test)]
    tests::ENDED_RECOVERY.set(Some(open));
    rsx! {
        span { class: "public-auction-catalog__actions-list",
            button {
                class: "public-auction-catalog__icon-action",
                r#type: "button",
                aria_haspopup: "dialog",
                aria_label: "{label}",
                title: "{label}",
                aria_describedby: "{reason_id}",
                onclick: move |_| open.call(()),
                CatalogActionIcon { kind: "gavel" }
            }
            span {
                id: "{reason_id}",
                class: "public-auction-catalog__leading-reason",
                role: "note",
                {status.entry_message()}
            }
        }
    }
}

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
    /// 该场冻结策略的代理默认模式，只决定代理新建表单的初始选项。
    proxy_default_mode: Option<String>,
    session_ready: bool,
    authenticated: bool,
    leading: bool,
    /// 详情抽屉出价入口的投影。本组件是出价资格判定的唯一 owner：行内按钮与抽屉按钮读取
    /// 同一份结果，卸载时失败关闭。
    detail_bid: CatalogDetailBidSlot,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let user_id = use_context::<commands::PublicCommandIdentity>().0;
    let session = use_context::<commands::CommandSession>();
    let snapshots = use_context::<CatalogProxySnapshots>();
    let mut dialog = use_context::<CommandDialogContext>().0;
    let commands = try_use_context::<commands::CompetitionCommands>();
    let pending = use_memo(use_reactive((&lot_id,), move |(lot_id,)| {
        commands
            .and_then(commands::CompetitionCommands::current)
            .and_then(|store| store.command(&lot_id))
            .map(|command| command.status)
    }));
    let active_action = use_memo(use_reactive((&lot_id,), move |(lot_id,)| {
        dialog.read().as_ref().filter(|input| input.lot_id == lot_id).map(|input| input.action)
    }));
    // 详情抽屉打开本拍品时同样激活代理视图：抽屉要展示本人授权的上限与已用次数。memo 只在
    // “是否打开本拍品”变化时通知，抽屉快照随价格刷新不会重读。
    let detail_dialog = use_context::<CatalogDetailDialogContext>().0;
    let detail_open = use_memo(use_reactive((&lot_id,), move |(lot_id,)| {
        detail_dialog.read().as_ref().is_some_and(|input| input.row.lot_id == lot_id)
    }));
    let proxy_view_active = move || active_action() == Some(CatalogAction::Proxy) || detail_open();
    let proxy_state = use_resource(use_reactive((&lot_id,), {
        let session = session.clone();
        let snapshots = snapshots.clone();
        move |(lot_id,)| {
            // 只在本拍品详情或代理页签打开时订阅刷新，避免闭合行随全局刷新调度空任务。
            let should_read =
                proxy_view_active() && session.identity_confirmed() && !user_id.is_nil();
            if should_read {
                let _ = refresh_generation();
            }
            let snapshots = snapshots.clone();
            async move {
                if should_read {
                    let result = read_my_proxy(user_id, lot_id.clone()).await;
                    // 只有同一身份、场次内的实际读取能替换这件拍品的结果，关闭视图不使其失效。
                    snapshots.0.borrow_mut().insert(lot_id, result);
                }
            }
        }
    }));
    let identity_confirmed = session.identity_confirmed();
    let proxy_view_active = proxy_view_active() && identity_confirmed;
    use_bid_refresh(proxy_state, proxy_view_active);
    // 每行跟随自己的 Resource 更新，授权结果变化不通知其他拍品。
    let proxy_ready = *proxy_state.state().read() == UseResourceState::Ready;
    let proxy_snapshot =
        usable_proxy_snapshot(snapshots.0.borrow().get(&lot_id), identity_confirmed, proxy_ready)
            .cloned();
    let active_proxy = active_own_proxy(proxy_snapshot.as_ref());
    let has_active_proxy = active_proxy.is_some();
    let proxy_leading =
        active_proxy.and_then(|proxy| proxy.bid.as_ref()).is_some_and(|bid| bid.leading);
    let effective_leading = leading || proxy_leading;
    let limit_reached = next_bid > MAX_PUBLIC_BID_AMOUNT;
    let ordinary_blocked = ordinary_bid_blocked(limit_reached, effective_leading, has_active_proxy);
    let pending = pending();
    let has_pending = pending.is_some();
    let action_reason = if let Some(status) = pending {
        status.entry_message()
    } else if !can_bid {
        t!("bid-unavailable")
    } else if has_active_proxy {
        t!("proxy-active-exists")
    } else if limit_reached {
        t!("bid-limit-note")
    } else if effective_leading {
        t!("bid-leading")
    } else {
        String::new()
    };
    // 领先由装备高亮表达，行自身已带“正在领先”的描述，不再重复说明；超过金额上限等
    // 其余原因必须可见。不显示说明时按钮不引用它。
    let show_action_reason =
        has_pending || has_active_proxy || !(can_bid && effective_leading && !limit_reached);
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
        proxy_default_mode,
        ordinary_blocked,
        leading: effective_leading,
        blocked_reason: action_reason.clone(),
        proxy: proxy_snapshot,
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
    let command_entry = has_pending || (authenticated && identity_confirmed && can_bid);
    let login_entry = !authenticated && can_bid && session_ready && !limit_reached;
    let bid_disabled = !has_pending && (!can_bid || ordinary_blocked);
    let detail_projection = if command_entry {
        let entry_action = if has_pending {
            Some(CatalogAction::Bid)
        } else {
            detail_entry_action(bid_disabled, can_bid, effective_leading, has_active_proxy)
        };
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
    let CatalogDetailBidSlot(mut detail_slot) = detail_bid;
    use_effect(use_reactive((&detail_projection,), move |(next,)| {
        if *detail_slot.peek() != next {
            detail_slot.set(next);
        }
    }));
    let dropped_lot = lot_id;
    use_drop(move || {
        // GET 错误、分页或阶段切换可卸载行，但不能卸载页面级弹窗或冻结命令。
        if let Ok(mut current) = dialog.try_write()
            && let Some(input) = current.as_mut().filter(|input| input.lot_id == dropped_lot)
        {
            input.can_bid = false;
            input.proxy = None;
        }
        // 拍品结束后行仍在，但操作栏已卸载：抽屉入口随之失败关闭，不保留最后一次资格。
        if let Ok(mut current) = detail_slot.try_write() {
            *current = CatalogDetailBid::Unavailable;
        }
    });
    let bid_label = t!("catalog-bid-lot", code: input.lot_code.clone());
    let bid_input = input;

    rsx! {
        span { class: "public-auction-catalog__actions-list",
            // 目录行使用「查看 / 出价 / 标记」三个图标：查看入口属于行（任何阶段都有），
            // 这里只负责出价；代理在出价弹窗的页签与详情抽屉里。
            if command_entry {
                button {
                    class: "public-auction-catalog__icon-action",
                    r#type: "button",
                    aria_haspopup: "dialog",
                    aria_expanded: (active_action() == Some(CatalogAction::Bid)).to_string(),
                    disabled: bid_disabled,
                    // 图标按钮的可访问名称带拍品编号，逐行可区分；悬停提示保持简短动作名。
                    aria_label: bid_label,
                    title: t!("catalog-bid"),
                    aria_describedby: if bid_disabled || has_pending {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        // 已打开的模态命令不接受另一个入口替换负载或目标。
                        if !bid_disabled && dialog.peek().is_none() {
                            dialog.set(Some(bid_input.clone()));
                        }
                    },
                    CatalogActionIcon { kind: "gavel" }
                }
            } else if login_entry {
                Link {
                    class: "public-auction-catalog__icon-action",
                    to: login_href_for_current_location(),
                    aria_label: t!("catalog-login-to-bid"),
                    title: t!("catalog-login-to-bid"),
                    CatalogActionIcon { kind: "gavel" }
                }
            }
            if show_action_reason && (ordinary_blocked || has_pending) {
                span {
                    id: action_reason_id,
                    class: "public-auction-catalog__leading-reason",
                    role: "note",
                    "{action_reason}"
                }
            }
        }
    }
}

async fn read_my_proxy(user_id: Uuid, lot_id: String) -> ProxyRead {
    #[cfg(test)]
    if let Some(result) = tests::proxy_request(user_id, &lot_id) {
        return result;
    }
    fetch_my_proxy(&user_id.to_string(), lot_id).await
}

/// 只有读到本人的生效授权才提前限制普通出价；未知授权由服务端裁决。
pub(super) fn active_own_proxy(
    snapshot: Option<&Result<Option<OwnedProxy>, String>>,
) -> Option<&OwnedProxy> {
    match snapshot {
        Some(Ok(Some(proxy))) if proxy.status == "active" => Some(proxy),
        _ => None,
    }
}

fn ordinary_bid_blocked(limit_reached: bool, leading: bool, has_active_proxy: bool) -> bool {
    limit_reached || leading || has_active_proxy
}

/// 详情抽屉命令入口打开的页签；`None` 表示入口禁用。
///
/// 生效授权可直接进入代理管理。领先者也保留代理入口，授权尚未读到时由弹窗读取。
fn detail_entry_action(
    ordinary_disabled: bool,
    can_bid: bool,
    leading: bool,
    has_active_proxy: bool,
) -> Option<CatalogAction> {
    if !can_bid {
        return None;
    }
    if has_active_proxy || leading {
        Some(CatalogAction::Proxy)
    } else {
        (!ordinary_disabled).then_some(CatalogAction::Bid)
    }
}

/// 重试时保留已确认授权与资格提示，暂时收起上一次读取错误及重试入口。
fn usable_proxy_snapshot(
    snapshot: Option<&ProxyRead>,
    identity_confirmed: bool,
    ready: bool,
) -> Option<&ProxyRead> {
    snapshot.filter(|result| identity_confirmed && (ready || result.is_ok()))
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
pub(super) mod tests {
    use std::cell::{Cell, RefCell};

    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
    use dioxus::dioxus_core::{AttributeValue, ElementId, Mutation, Mutations};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
    use uuid::Uuid;

    use super::{
        CatalogAction, LiveCatalogActions, ProxyRead, active_own_proxy, detail_entry_action,
        ordinary_bid_blocked, usable_proxy_snapshot, use_catalog_proxy_snapshots,
    };
    use crate::api::{CatalogRow, OwnedProxy, PublicSession};
    use crate::surface::catalog::command_scope::CatalogCommandScope;
    use crate::surface::catalog::detail::{
        CatalogDetailBid, CatalogDetailBidSlot, CatalogDetailDialogContext, CatalogDetailInput,
    };
    use crate::surface::router::PUBLIC_LOCALE_CATALOG;
    use crate::surface::watches::{LotWatchScope, SkipWatchReads};
    use crate::surface::{PublicBidRefresh, PublicCommandOwner, PublicSessionResource};

    const ALICE: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
    const BOB: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000002";
    const AUCTION: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000005";
    const LOT: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000006";
    const ACTIVE_REASON: &str = "An active proxy authorization already exists for this lot.";

    struct ProxyReads {
        authorization: Option<OwnedProxy>,
        requests: Vec<(Uuid, String)>,
        owner: Option<Signal<Option<String>>>,
        row: Option<Signal<bool>>,
        detail: Option<Signal<Option<CatalogDetailInput>>>,
    }

    thread_local! {
        static PROXY_READS: RefCell<Option<ProxyReads>> = const { RefCell::new(None) };
        pub(in crate::surface::catalog) static ENDED_RECOVERY: Cell<Option<EventHandler<()>>> = const { Cell::new(None) };
    }

    pub(super) fn proxy_request(user_id: Uuid, lot_id: &str) -> Option<ProxyRead> {
        PROXY_READS.with_borrow_mut(|reads| {
            let reads = reads.as_mut()?;
            reads.requests.push((user_id, lot_id.to_owned()));
            Some(Ok(reads.authorization.clone()))
        })
    }

    #[component]
    fn CatalogFixture() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_PUBLIC_LOCALE,
                PUBLIC_LOCALE_CATALOG.default_ftl,
                &DEFAULT_PUBLIC_LOCALE,
                None,
            )
        })
        .expect("加载公开端英文译文");
        let owner = use_signal(|| Some(ALICE.to_owned()));
        let session: PublicSessionResource = use_resource(move || {
            let identity = owner();
            async move {
                Ok(identity.map(|user_id| PublicSession {
                    authenticated: true,
                    username: user_id.clone(),
                    user_id,
                    expires_at: None,
                    absolute_expires_at: None,
                }))
            }
        });
        use_context_provider(|| session);
        use_context_provider(|| PublicCommandOwner(owner));
        use_context_provider(|| Signal::new(0_u64));
        use_context_provider(|| PublicBidRefresh(Signal::new(0_u64)));
        use_context_provider(|| SkipWatchReads);
        PROXY_READS.with_borrow_mut(|reads| reads.as_mut().unwrap().owner = Some(owner));
        rsx! {
            CatalogCommandScope {
                LotWatchScope { auction_id: AUCTION.to_owned(), CatalogRowFixture {} }
            }
        }
    }

    #[component]
    fn CatalogRowFixture() -> Element {
        use_catalog_proxy_snapshots();
        let detail = use_context::<CatalogDetailDialogContext>().0;
        let row = use_signal(|| true);
        let bid = use_signal(CatalogDetailBid::default);
        PROXY_READS.with_borrow_mut(|reads| {
            let reads = reads.as_mut().unwrap();
            reads.row = Some(row);
            reads.detail = Some(detail);
        });
        rsx! {
            if row() {
                LiveCatalogActions {
                    lot_id: LOT.to_owned(), lot_code: "S07".to_owned(),
                    lot_name: "Legendary Shield".to_owned(),
                    can_bid: true, current_price: 110_000, next_bid: 115_000,
                    lot_status: "live".to_owned(), allow_slow: true,
                    proxy_default_mode: Some("slow".to_owned()),
                    session_ready: true, authenticated: true, leading: false,
                    detail_bid: CatalogDetailBidSlot(bid),
                }
            }
        }
    }

    fn settle(dom: &mut VirtualDom, mutations: &mut Mutations) {
        for _ in 0..16 {
            dom.process_events();
            dom.render_immediate(mutations);
        }
    }

    fn mounted() -> VirtualDom {
        PROXY_READS.with_borrow_mut(|reads| {
            *reads = Some(ProxyReads {
                authorization: Some(waiting_proxy()),
                requests: Vec::new(),
                owner: None,
                row: None,
                detail: None,
            });
        });
        let mut dom = VirtualDom::new(CatalogFixture);
        let mut mutations = Mutations::default();
        dom.rebuild(&mut mutations);
        settle(&mut dom, &mut mutations);
        assert!(bid_button(&mutations, false).is_some());
        assert_eq!(requests(), 0, "目录行挂载时不预取授权");
        dom
    }

    fn show_detail(dom: &mut VirtualDom, open: bool) -> Mutations {
        let mut detail = PROXY_READS.with_borrow(|reads| reads.as_ref().unwrap().detail.unwrap());
        dom.in_runtime(|| {
            detail.set(open.then(|| CatalogDetailInput {
                row: CatalogRow {
                    lot_id: LOT.to_owned(),
                    code: "S07".to_owned(),
                    name: "Legendary Shield".to_owned(),
                    game_equipment_url: String::new(),
                    category: "shield".to_owned(),
                    equipment_group: "shield".to_owned(),
                    status: "live".to_owned(),
                    seller_name: String::new(),
                    level: None,
                    primary_attribute_initials: None,
                    avg_roll: None,
                    start_price: 100_000,
                    current_price: 110_000,
                    next_bid: 115_000,
                    leader_name: None,
                    remaining: String::new(),
                },
                bid: CatalogDetailBid::Unavailable,
                watchable: false,
            }));
        });
        let mut mutations = Mutations::default();
        settle(dom, &mut mutations);
        mutations
    }

    fn bid_button(mutations: &Mutations, disabled: bool) -> Option<ElementId> {
        mutations.edits.iter().find_map(|edit| match edit {
            Mutation::SetAttribute {
                name: "disabled",
                value: AttributeValue::Bool(value),
                id,
                ..
            } if *value == disabled => Some(*id),
            _ => None,
        })
    }

    fn shows_proxy_reason(mutations: &Mutations) -> bool {
        mutations.edits.iter().any(|edit| {
            matches!(edit,
                Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                if value == ACTIVE_REASON
            )
        })
    }

    fn requests() -> usize {
        PROXY_READS.with_borrow(|reads| reads.as_ref().unwrap().requests.len())
    }

    #[test]
    fn closing_details_keeps_the_row_disabled_and_preserves_its_reason() {
        let mut dom = mounted();
        let opened = show_detail(&mut dom, true);
        assert_eq!(requests(), 1);
        assert!(bid_button(&opened, true).is_some());
        assert!(shows_proxy_reason(&opened));
        let reason_id = format!("public-auction-action-reason-{LOT}");
        let reason = opened
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::SetAttribute {
                    name: "id",
                    value: AttributeValue::Text(value),
                    id,
                    ..
                } if value == &reason_id => Some(*id),
                _ => None,
            })
            .expect("禁用理由已渲染");

        let closed = show_detail(&mut dom, false);
        assert!(bid_button(&closed, false).is_none(), "关闭抽屉后普通出价仍禁用");
        assert!(
            !closed.edits.iter().any(|edit| matches!(edit,
                Mutation::Remove { id } | Mutation::ReplaceWith { id, .. } if *id == reason
            )),
            "关闭抽屉后保留行内理由"
        );

        let mut row = PROXY_READS.with_borrow(|reads| reads.as_ref().unwrap().row.unwrap());
        dom.in_runtime(|| row.set(false));
        settle(&mut dom, &mut Mutations::default());
        dom.in_runtime(|| row.set(true));
        let mut remounted = Mutations::default();
        settle(&mut dom, &mut remounted);
        assert!(bid_button(&remounted, true).is_some());
        assert!(shows_proxy_reason(&remounted));
        assert_eq!(requests(), 1, "关抽屉和重挂载行都不发授权 GET");

        PROXY_READS.with_borrow_mut(|reads| reads.as_mut().unwrap().authorization = None);
        let refreshed = show_detail(&mut dom, true);
        assert_eq!(requests(), 2);
        assert!(bid_button(&refreshed, false).is_some(), "新读取的无授权结果解除禁用");
    }

    #[test]
    fn an_identity_change_discards_the_previous_owners_authorization() {
        let mut dom = mounted();
        let opened = show_detail(&mut dom, true);
        assert!(bid_button(&opened, true).is_some());
        assert!(shows_proxy_reason(&opened));
        show_detail(&mut dom, false);
        let mut owner = PROXY_READS.with_borrow(|reads| reads.as_ref().unwrap().owner.unwrap());
        dom.in_runtime(|| owner.set(Some(BOB.to_owned())));
        let mut switched = Mutations::default();
        settle(&mut dom, &mut switched);
        assert!(bid_button(&switched, false).is_some());
        assert!(!shows_proxy_reason(&switched));
        assert_eq!(requests(), 1, "换号不预取，也不沿用上一用户授权");

        PROXY_READS.with_borrow_mut(|reads| reads.as_mut().unwrap().authorization = None);
        let reopened = show_detail(&mut dom, true);
        assert!(bid_button(&reopened, true).is_none());
        assert!(!shows_proxy_reason(&reopened));
        PROXY_READS.with_borrow(|reads| {
            let requests = &reads.as_ref().unwrap().requests;
            assert_eq!(
                requests,
                &[
                    (Uuid::parse_str(ALICE).unwrap(), LOT.to_owned()),
                    (Uuid::parse_str(BOB).unwrap(), LOT.to_owned()),
                ],
            );
        });
    }

    pub(in crate::surface::catalog) fn waiting_proxy() -> OwnedProxy {
        serde_json::from_value(serde_json::json!({
            "authorization_id": "018f4c8e-7b2a-4c3d-8e4f-000000000004",
            "auction_id": "018f4c8e-7b2a-4c3d-8e4f-000000000005",
            "lot_id": "018f4c8e-7b2a-4c3d-8e4f-000000000006",
            "lot_number": 7,
            "authorization_sequence": 1,
            "usage_number": 2,
            "mode": "slow",
            "maximum_amount": 250_000,
            "quota": { "used": 2, "limit": 3 },
            "transitions": [],
            "status": "active",
            "state_version": 4,
            "created_at": "2026-11-13T00:00:00Z",
            "updated_at": "2026-11-13T00:00:00Z",
            "equipment": { "name": "Legendary Shield", "category": "shield" },
            "capabilities": { "can_lower": true, "waiting": true, "at_maximum": false },
            "bid": {
                "id": "018f4c8e-7b2a-4c3d-8e4f-000000000007",
                "auction_id": "018f4c8e-7b2a-4c3d-8e4f-000000000005",
                "lot_id": "018f4c8e-7b2a-4c3d-8e4f-000000000006",
                "lot_number": 7,
                "amount": 105_000,
                "accepted_state_version": 3,
                "fact_index": 1,
                "mechanism": "proxy",
                "created_at": "2026-11-13T00:00:00Z",
                "leading": false,
                "equipment": { "name": "Legendary Shield", "category": "shield" }
            }
        }))
        .expect("等待反击的授权响应")
    }

    #[test]
    fn detail_entry_keeps_proxy_management_reachable_for_leading_bidders() {
        // 普通出价可用：进直接出价，代理由弹窗页签切换。
        assert!(detail_entry_action(false, true, false, false) == Some(CatalogAction::Bid));
        // 回归：领先者的出价入口被禁用后，仍要能进入代理页签降低已有授权。
        assert!(detail_entry_action(true, true, true, false) == Some(CatalogAction::Proxy));
        // 达到金额上限的非领先者没有可执行的操作；拍品不可竞价时一律禁用。
        assert!(detail_entry_action(true, true, false, false).is_none());
        assert!(detail_entry_action(true, false, true, false).is_none());
        assert!(detail_entry_action(true, false, false, false).is_none());
    }

    #[test]
    fn active_waiting_proxy_blocks_ordinary_bid_but_allows_management() {
        let snapshot = Ok(Some(waiting_proxy()));
        let active_proxy = active_own_proxy(Some(&snapshot));
        assert!(active_proxy.is_some_and(|proxy| proxy.capabilities.waiting));
        let leading =
            active_proxy.and_then(|proxy| proxy.bid.as_ref()).is_some_and(|bid| bid.leading);
        let has_active_proxy = active_proxy.is_some();
        let ordinary_blocked = ordinary_bid_blocked(false, leading, has_active_proxy);
        assert!(ordinary_blocked);
        assert!(
            detail_entry_action(ordinary_blocked, true, leading, has_active_proxy)
                == Some(CatalogAction::Proxy)
        );
        assert!(detail_entry_action(ordinary_blocked, false, leading, has_active_proxy).is_none());
    }

    #[test]
    fn missing_or_failed_proxy_reads_leave_ordinary_bid_available() {
        for snapshot in [None, Some(Ok(None)), Some(Err(String::from("network_error")))] {
            let has_active_proxy = active_own_proxy(snapshot.as_ref()).is_some();
            let ordinary_blocked = ordinary_bid_blocked(false, false, has_active_proxy);
            assert!(!ordinary_blocked);
            assert!(
                detail_entry_action(ordinary_blocked, true, false, has_active_proxy)
                    == Some(CatalogAction::Bid)
            );
        }
    }

    #[test]
    fn finished_read_is_usable_by_the_confirmed_identity() {
        let success = Ok(Some(waiting_proxy()));
        assert!(usable_proxy_snapshot(Some(&success), true, true).is_some());
        // 读取失败也要交给表单，由它展示错误与重试入口。
        let failure = Err("network_error".to_owned());
        assert!(usable_proxy_snapshot(Some(&failure), true, true).is_some());
        assert!(usable_proxy_snapshot(Some(&success), false, true).is_none());
    }

    #[test]
    fn refresh_keeps_the_last_successful_snapshot() {
        // 回归：竞价事件触发的刷新不能卸载正在填写的代理表单。
        let success = Ok(Some(waiting_proxy()));
        assert!(usable_proxy_snapshot(Some(&success), true, false).is_some());
    }

    #[test]
    fn a_snapshot_requires_an_actual_read() {
        assert!(usable_proxy_snapshot(None, true, true).is_none());
        assert!(usable_proxy_snapshot(None, true, false).is_none());
    }

    #[test]
    fn retry_hides_the_previous_read_error() {
        let failure = Err("network_error".to_owned());
        assert!(usable_proxy_snapshot(Some(&failure), true, false).is_none());
    }
}
