//! 目录之外按拍品 ID 打开的只读详情抽屉：账户页这类列表只有拍品 ID，没有目录行快照。
//! 先读取拍品详情，就绪后才渲染抽屉，避免空外壳滑入后再换内容；抽屉不带出价与标记入口，
//! 出价资格仍只由目录操作栏在拍卖作用域内判定。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{CatalogRow, catalog_row_from_detail, fetch_lot_detail};
use crate::surface::catalog::actions::CatalogActionIcon;
use crate::surface::use_bid_refresh;

use super::drawer::CatalogDetailDrawer;
use super::{CatalogDetailBid, CatalogDetailInput, DetailWorld};

/// 一次查看请求指向的拍品。`auction_id` 用于抽屉内的竞价历史读取；`world` 是入口已知的
/// 拍品所在世界，未知时价格历史按拍卖读取。
#[derive(Clone, PartialEq)]
struct LotDetailTarget {
    auction_id: String,
    lot_id: String,
    world: Option<String>,
}

#[derive(Clone, PartialEq)]
enum LookupState {
    Closed,
    /// 首次读取中：抽屉尚未渲染，行内按钮显示忙碌。
    Loading(LotDetailTarget),
    Open(LotDetailTarget),
    /// 首次读取失败或拍品已不存在；下一次查看请求会覆盖它。
    Failed,
}

impl LookupState {
    /// 新的查看请求。同一拍品读取中或已展示时返回 `None`，重复点击不重启读取。
    fn requested(&self, target: LotDetailTarget) -> Option<Self> {
        match self {
            Self::Loading(current) | Self::Open(current) if *current == target => None,
            _ => Some(Self::Loading(target)),
        }
    }

    /// 读取完成后的迁移。结果只对仍在等待的同一拍品生效：已展示时的后台重读失败保留
    /// 上一次快照，已关闭或已改看其他拍品的迟到结果直接丢弃。
    fn resolved(&self, lot_id: &str, loaded: bool) -> Option<Self> {
        match self {
            Self::Loading(target) if target.lot_id == lot_id => {
                Some(if loaded { Self::Open(target.clone()) } else { Self::Failed })
            }
            _ => None,
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

    /// 最近一次查看请求是否读取失败。
    pub(crate) fn failed(self) -> bool {
        matches!(&*self.0.read(), LookupState::Failed)
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
            aria_label: t!("catalog-view-lot", code: lot_code),
            title: t!("catalog-view"),
            onclick: move |_| {
                lookup
                    .open(LotDetailTarget {
                        auction_id: auction_id.clone(),
                        lot_id: lot_id.clone(),
                        world: world.clone(),
                    });
            },
            CatalogActionIcon { kind: "eye" }
        }
    }
}

/// 只读详情抽屉的宿主。以拍品为 key：改看另一件时整体重建，上一件的读取随之取消。
#[component]
pub(crate) fn LotDetailLookupHost(lookup: LotDetailLookup) -> Element {
    let target = match &*lookup.0.read() {
        LookupState::Loading(target) | LookupState::Open(target) => Some(target.clone()),
        LookupState::Closed | LookupState::Failed => None,
    };

    rsx! {
        for target in target {
            LotDetailLookupSession { key: "{target.lot_id}", target, lookup }
        }
    }
}

/// 一件拍品从读取到关闭的生命周期。
#[component]
fn LotDetailLookupSession(target: LotDetailTarget, lookup: LotDetailLookup) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut state = lookup.0;
    let mut shown = use_signal(|| None::<CatalogRow>);
    let detail = use_resource(use_reactive((&target.lot_id,), move |(lot_id,)| {
        // 本人命令成功后推进的刷新代数；他人出价由下方的竞价通知补读。
        let _ = refresh_generation();
        async move {
            let row = fetch_lot_detail(lot_id.clone())
                .await
                .ok()
                .and_then(|page| page.lot)
                .map(catalog_row_from_detail);
            let loaded = row.is_some();
            if let Some(row) = row {
                shown.set(Some(row));
            }
            // 读取任务处于响应式上下文内，这里只窥视状态，避免把迁移结果订阅成重读依赖。
            let next = state.peek().resolved(&lot_id, loaded);
            if let Some(next) = next {
                state.set(next);
            }
        }
    }));
    use_bid_refresh(detail, true);
    let Some(row) = shown() else {
        return rsx! {};
    };

    rsx! {
        CatalogDetailDrawer {
            auction_id: target.auction_id.clone(),
            input: CatalogDetailInput {
                row,
                bid: CatalogDetailBid::Unavailable,
                watchable: false,
            },
            world: target.world.clone().map_or(DetailWorld::Auction, DetailWorld::Known),
            on_close: move |_| state.set(LookupState::Closed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LookupState, LotDetailTarget};

    fn target(lot_id: &str) -> LotDetailTarget {
        LotDetailTarget {
            auction_id: "auction-1".to_owned(),
            lot_id: lot_id.to_owned(),
            world: None,
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
            LookupState::Failed,
            LookupState::Loading(target("lot-a")),
            LookupState::Open(target("lot-a")),
        ] {
            assert!(
                state.requested(target("lot-b")) == Some(LookupState::Loading(target("lot-b")))
            );
        }
    }

    #[test]
    fn first_read_decides_between_open_and_failed() {
        let loading = LookupState::Loading(target("lot-a"));
        assert!(loading.resolved("lot-a", true) == Some(LookupState::Open(target("lot-a"))));
        assert!(loading.resolved("lot-a", false) == Some(LookupState::Failed));
    }

    #[test]
    fn late_or_background_results_never_change_the_state() {
        // 已展示时的后台重读失败保留抽屉；已关闭或已改看其他拍品的结果被丢弃。
        assert!(LookupState::Open(target("lot-a")).resolved("lot-a", false).is_none());
        assert!(LookupState::Closed.resolved("lot-a", true).is_none());
        assert!(LookupState::Failed.resolved("lot-a", true).is_none());
        assert!(LookupState::Loading(target("lot-b")).resolved("lot-a", true).is_none());
    }
}
