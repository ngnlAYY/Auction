//! 目录之外按拍品 ID 打开的只读详情抽屉：账户页这类列表只有拍品 ID，没有目录行快照。
//! 先读取拍品详情，就绪后才渲染抽屉，避免空外壳滑入后再换内容；抽屉不带出价与标记入口，
//! 出价资格仍只由目录操作栏在拍卖作用域内判定。

use std::cell::Cell;
use std::rc::Rc;

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::deep_link::lot_view_entry_is_current;
use super::drawer::CatalogDetailDrawer;
use super::{CatalogDetailBid, CatalogDetailInput, DetailWorld, LotViewTarget};
use crate::api::{CatalogRow, LotDetailFetchError, catalog_row_from_detail, fetch_lot_detail};
use crate::surface::catalog::actions::CatalogActionIcon;
use crate::surface::use_auction_bid_refresh;

/// 一次查看请求指向的拍品。`auction_id` 用于抽屉内的竞价历史读取；`world` 是入口已知的
/// 拍品所在世界，未知时价格历史按拍卖读取。
#[derive(Clone, PartialEq)]
struct LotDetailTarget {
    auction_id: String,
    lot_id: String,
    world: Option<String>,
    lot_code: String,
    entry: Option<u64>,
}

#[derive(Clone, PartialEq)]
struct LotDetailSeed {
    row: Rc<CatalogRow>,
    generation: u64,
}

#[derive(Clone, PartialEq)]
enum LookupState {
    Closed,
    /// 首次读取中：抽屉尚未渲染，行内按钮显示忙碌。
    Loading(LotDetailTarget),
    Open(LotDetailTarget),
    /// 深链已读到权威详情，直接展示并参与后续实时重读。
    Seeded(LotDetailTarget, LotDetailSeed),
    /// 保留读取目标，明确不存在与可补读的错误不能互相替代。
    Failed(LotDetailTarget, LotDetailFetchError),
}

impl LookupState {
    /// 新的查看请求。同一拍品读取中或已展示时返回 `None`，重复点击不重启读取。
    fn requested(&self, target: LotDetailTarget) -> Option<Self> {
        match self {
            Self::Loading(current) | Self::Open(current) | Self::Seeded(current, _)
                if *current == target =>
            {
                None
            }
            _ => Some(Self::Loading(target)),
        }
    }

    fn reading_target(&self) -> Option<&LotDetailTarget> {
        match self {
            Self::Loading(target) | Self::Open(target) | Self::Seeded(target, _) => Some(target),
            Self::Closed | Self::Failed(_, _) => None,
        }
    }
}

/// 查看请求的句柄。行组件只写状态、不持有读取任务：任务归宿主所有，行随分页卸载时
/// 读取不会被取消，忙碌态也不会卡住。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct LotDetailLookup(Signal<LookupState>);

impl LotDetailLookup {
    fn open(mut self, target: LotDetailTarget) {
        let next = self.0.peek().requested(target);
        if let Some(next) = next {
            self.0.set(next);
        }
    }

    /// 沿用深链已验证的详情；空看板和目录错误不需要再次读取才能打开抽屉。
    pub(crate) fn view(mut self, target: LotViewTarget) {
        self.0.set(LookupState::Seeded(
            LotDetailTarget {
                auction_id: target.auction_id,
                lot_id: target.lot_id,
                world: target.world,
                lot_code: target.row.code.clone(),
                entry: target.entry,
            },
            LotDetailSeed { row: target.row, generation: target.generation },
        ));
    }
}

/// 建立查找状态并提供给后代行组件。调用方须把 [`LotDetailLookupHost`] 渲染在不随分页
/// 或重读卸载的位置。
pub(crate) fn use_lot_detail_lookup() -> LotDetailLookup {
    use_context_provider(|| LotDetailLookup(Signal::new(LookupState::Closed)))
}

/// 列表行的查看按钮：外观与目录操作栏的查看入口一致。读取期间只标记忙碌而不禁用，
/// 抽屉关闭后焦点才能回到这枚按钮；重复点击由状态迁移忽略。
#[component]
pub(crate) fn LotDetailLookupButton(
    auction_id: String,
    lot_id: String,
    lot_code: String,
    /// 列表已知拍品所在世界时传入；账户列表只有拍卖 UUID，留空。
    world: Option<String>,
) -> Element {
    let lookup = use_context::<LotDetailLookup>();
    let busy = matches!(
        &*lookup.0.read(),
        LookupState::Loading(target) if target.lot_id == lot_id
    );

    rsx! {
        button {
            class: "public-lot-view",
            r#type: "button",
            aria_haspopup: "dialog",
            aria_busy: busy.to_string(),
            // 可访问名称带拍品编号，逐行可区分；悬停提示保持简短动作名。
            aria_label: t!("catalog-view-lot", code: lot_code.clone()),
            title: t!("catalog-view"),
            onclick: move |_| {
                lookup
                    .open(LotDetailTarget {
                        auction_id: auction_id.clone(),
                        lot_id: lot_id.clone(),
                        world: world.clone(),
                        lot_code: lot_code.clone(),
                        entry: None,
                    });
            },
            CatalogActionIcon { kind: "eye" }
        }
    }
}

/// 只读详情抽屉的宿主。以拍品为 key：改看另一件时整体重建，上一件的读取随之取消。
#[component]
pub(crate) fn LotDetailLookupHost(lookup: LotDetailLookup) -> Element {
    let mut state = lookup.0;
    let router = dioxus::router::try_router();
    use_effect(move || {
        if let Some(router) = router {
            let _ = router.full_route_string();
        }
        let entry = match &*state.read() {
            LookupState::Failed(target, _) => target.entry,
            current => current.reading_target().and_then(|target| target.entry),
        };
        if !lot_view_entry_is_current(entry) {
            state.set(LookupState::Closed);
        }
    });
    let snapshot = state.read();
    let target = match &*snapshot {
        LookupState::Loading(target) | LookupState::Open(target) => Some((target.clone(), None)),
        LookupState::Seeded(target, seed) => Some((target.clone(), Some(seed.clone()))),
        LookupState::Closed | LookupState::Failed(_, _) => None,
    };
    let failure = match &*snapshot {
        LookupState::Failed(target, error) if lot_view_entry_is_current(target.entry) => {
            Some((target.clone(), *error))
        }
        _ => None,
    };
    let seed_generation =
        target.as_ref().and_then(|(_, seed)| seed.as_ref()).map(|seed| seed.generation);

    rsx! {
        if let Some((target, seed)) = target {
            LotDetailLookupSession {
                key: "{target.auction_id}:{target.lot_id}:{target.entry:?}:{seed_generation:?}",
                target,
                lookup,
                seed,
            }
        }
        if let Some((target, error)) = failure {
            if error.not_found() {
                AsyncState {
                    kind: AsyncStateKind::Empty,
                    title: t!("catalog-view-lot", code: target.lot_code),
                    detail: t!("catalog-detail-not-found"),
                }
            } else {
                div { class: "ui-async-state ui-async-state--error",
                    div { role: "alert",
                        p { class: "ui-async-state__title", {t!("catalog-view-lot", code: target.lot_code.clone())} }
                        p { id: "public-lot-detail-read-error", class: "ui-async-state__detail", {t!("data-error")} }
                    }
                    button {
                        id: "public-lot-detail-retry",
                        class: "ui-async-state__retry",
                        r#type: "button",
                        aria_describedby: "public-lot-detail-read-error",
                        onclick: move |_| {
                            if lot_view_entry_is_current(target.entry) {
                                lookup.open(target.clone());
                            }
                        },
                        {t!("data-retry")}
                    }
                }
            }
        }
    }
}

/// 一件拍品从读取到关闭的生命周期。
#[component]
fn LotDetailLookupSession(
    target: LotDetailTarget,
    lookup: LotDetailLookup,
    seed: Option<LotDetailSeed>,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut state = lookup.0;
    let seed = seed.filter(|seed| seed.generation == *refresh_generation.peek());
    let seed_generation =
        use_hook(|| Rc::new(Cell::new(seed.as_ref().map(|seed| seed.generation))));
    let mut shown = use_signal(|| seed);
    let detail = use_resource(use_reactive((&target,), move |(target,)| {
        // 本人命令成功后推进的刷新代数；他人出价由下方的竞价通知补读。
        let generation = refresh_generation();
        let skip_read = seed_generation.take() == Some(generation);
        async move {
            if skip_read || !lot_view_entry_is_current(target.entry) {
                return;
            }
            let result = fetch_lot_detail(target.lot_id.clone()).await.and_then(|page| {
                if page.auction_id != target.auction_id {
                    return Err(LotDetailFetchError::Unavailable);
                }
                page.lot.map(catalog_row_from_detail).ok_or(LotDetailFetchError::Unavailable)
            });
            // 命令失效之前发出的读取不能发布旧价格；离开目标历史条目后也不能弹出抽屉。
            if generation != *refresh_generation.peek()
                || !lot_view_entry_is_current(target.entry)
                || state.peek().reading_target() != Some(&target)
            {
                return;
            }
            match result {
                Ok(row) => {
                    shown.set(Some(LotDetailSeed { row: Rc::new(row), generation }));
                    if matches!(&*state.peek(), LookupState::Loading(_)) {
                        state.set(LookupState::Open(target));
                    }
                }
                // 命令失效后，补读失败不能继续展示提交前的价格与领先者。
                Err(error)
                    if error.not_found()
                        || shown
                            .peek()
                            .as_ref()
                            .is_none_or(|shown| shown.generation != generation) =>
                {
                    state.set(LookupState::Failed(target, error));
                }
                Err(_error) => {}
            }
        }
    }));
    use_auction_bid_refresh(detail, Some(&target.auction_id));
    let Some(shown) = shown() else {
        return rsx! {};
    };

    rsx! {
        CatalogDetailDrawer {
            auction_id: target.auction_id.clone(),
            input: CatalogDetailInput {
                row: (*shown.row).clone(),
                bid: CatalogDetailBid::Unavailable,
                watchable: false,
            },
            world: target.world.map_or(DetailWorld::Auction, DetailWorld::Known),
            on_close: move |_| state.set(LookupState::Closed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LookupState, LotDetailTarget};
    use crate::api::LotDetailFetchError;

    fn target(lot_id: &str) -> LotDetailTarget {
        LotDetailTarget {
            auction_id: "auction-1".to_owned(),
            lot_id: lot_id.to_owned(),
            world: None,
            lot_code: "One01".to_owned(),
            entry: None,
        }
    }

    #[test]
    fn repeated_request_for_the_same_lot_does_not_restart_the_read() {
        assert!(LookupState::Loading(target("lot-a")).requested(target("lot-a")).is_none());
        assert!(LookupState::Open(target("lot-a")).requested(target("lot-a")).is_none());
    }

    #[test]
    fn a_new_request_replaces_any_other_state() {
        for state in [
            LookupState::Closed,
            LookupState::Failed(target("lot-a"), LotDetailFetchError::Unavailable),
            LookupState::Loading(target("lot-a")),
            LookupState::Open(target("lot-a")),
        ] {
            assert!(
                state.requested(target("lot-b")) == Some(LookupState::Loading(target("lot-b")))
            );
        }
    }

    #[test]
    fn only_loading_or_open_targets_accept_results() {
        let loading = LookupState::Loading(target("lot-a"));
        assert!(loading.reading_target() == Some(&target("lot-a")));
        assert!(loading.reading_target() != Some(&target("lot-b")));
        assert!(LookupState::Closed.reading_target().is_none());
        assert!(
            LookupState::Failed(target("lot-a"), LotDetailFetchError::Missing)
                .reading_target()
                .is_none()
        );
    }
}
