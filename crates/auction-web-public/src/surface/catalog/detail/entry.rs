//! 目录行进入详情抽屉的入口：整行点击、行聚焦后的 Enter/Space 与操作栏查看按钮共用同一
//! 打开动作；行内自带交互的控件（装备链接、出价、标记、可展开的截断文本）保持各自行为。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{
    CatalogDetailBid, CatalogDetailBidSlot, CatalogDetailDialogContext, CatalogDetailInput,
    LotViewRequest,
};
use crate::api::CatalogRow;
use crate::surface::catalog::actions::CatalogActionIcon;
use crate::surface::catalog::command_scope::CommandDialogContext;
use crate::surface::is_activation_key;

/// 行本身与行内自带交互的区域。事件目标向上最近的匹配项是行时，事件才属于“整行”；
/// 操作栏整体算控件区，图标之间的空隙点偏了也不会改成打开抽屉。
#[cfg(target_arch = "wasm32")]
const ROW_OR_CONTROL_SELECTOR: &str = "a[href], button, input, select, textarea, summary, [tabindex], \
     .public-auction-catalog__action-controls, tr";

/// 一行的详情入口。`bid` 由该行操作栏写入，行只负责把它连同行快照交给抽屉。
#[derive(Clone, Copy)]
pub(in crate::surface::catalog) struct CatalogDetailEntry {
    /// 出价入口投影；没有操作栏的行（预览、已结算、已结束拍品）保持 `Unavailable`。
    pub bid: CatalogDetailBidSlot,
    /// 以当前行快照打开抽屉。
    pub open: Callback<()>,
}

impl CatalogDetailEntry {
    /// 整行点击：落在行内控件上的点击与结束文本拖选的点击都不打开抽屉。
    pub(in crate::surface::catalog) fn click(self, event: &Event<MouseData>) {
        #[cfg(target_arch = "wasm32")]
        {
            let on_row = event
                .data()
                .downcast::<web_sys::MouseEvent>()
                .is_some_and(|event| targets_row(event.target()));
            if !on_row || selecting_text() {
                return;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = event;
        self.open.call(());
    }

    /// 行自身聚焦时的 Enter/Space；行内控件冒泡上来的按键仍归控件自己。
    pub(in crate::surface::catalog) fn keydown(self, event: &Event<KeyboardData>) {
        if !is_activation_key(&event.key()) {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let on_row = event
                .data()
                .downcast::<web_sys::KeyboardEvent>()
                .is_some_and(|event| targets_row(event.target()));
            if !on_row {
                return;
            }
        }
        // 行聚焦时 Space 的默认行为是滚动页面。
        event.prevent_default();
        // 长按产生的重复按键不再次打开：抽屉被关闭、焦点回到本行后，同一次长按不会把它重新弹出。
        if event.is_auto_repeating() {
            return;
        }
        self.open.call(());
    }
}

/// 建立一行的详情入口，并让已打开的抽屉跟随同一拍品的行投影。
/// `watchable` 与行内是否渲染标记按钮保持同一条件。
pub(in crate::surface::catalog) fn use_catalog_detail_entry(
    row: &CatalogRow,
    watchable: bool,
) -> CatalogDetailEntry {
    let mut detail = use_context::<CatalogDetailDialogContext>().0;
    let command_dialog = use_context::<CommandDialogContext>().0;
    let bid = use_signal(CatalogDetailBid::default);
    // 行是抽屉快照的唯一写入方：打开中的抽屉随实时价格、领先者与出价资格更新，
    // 抽屉自身不订阅目录、不重算资格，因此不会停在点击瞬间的旧价格上。
    use_effect(use_reactive((row, &watchable), move |(row, watchable)| {
        let bid = bid();
        let current = detail.peek().clone();
        if let Some(current) = current.filter(|current| current.row.lot_id == row.lot_id) {
            let next = CatalogDetailInput { row, bid, watchable };
            if next != current {
                detail.set(Some(next));
            }
        }
    }));
    let dropped_lot = row.lot_id.clone();
    use_drop(move || {
        // 分页、筛选或目录读取失败会卸载行：抽屉保留最后一次展示快照，但出价入口失败关闭，
        // 不留下一份无人更新的资格。身份宿主先于行销毁时信号已不可写，直接跳过。
        if let Ok(mut current) = detail.try_write()
            && let Some(input) = current.as_mut().filter(|input| input.row.lot_id == dropped_lot)
        {
            input.bid = CatalogDetailBid::Unavailable;
        }
    });
    let open_row = row.clone();
    let open = use_callback(move |_: ()| {
        // 模态命令弹窗打开期间不在其下方再开抽屉。
        if command_dialog.peek().is_some() {
            return;
        }
        detail.set(Some(CatalogDetailInput {
            row: open_row.clone(),
            bid: bid.peek().clone(),
            watchable,
        }));
    });
    // 拍品直达链接：请求指向本行时由本行打开抽屉并清空请求。行是抽屉快照的唯一写入方，
    // 这样打开的抽屉与点击本行完全一样，出价入口照常跟随操作栏投影。
    let mut view_request = use_context::<LotViewRequest>().0;
    let requested_lot = row.lot_id.clone();
    use_effect(move || {
        let requested =
            view_request.read().as_ref().is_some_and(|target| target.lot_id == requested_lot);
        if requested {
            view_request.set(None);
            open.call(());
        }
    });
    CatalogDetailEntry { bid: CatalogDetailBidSlot(bid), open }
}

/// 操作栏的查看入口：任何阶段的目录行都有，与整行点击打开同一个抽屉。
#[component]
pub(in crate::surface::catalog) fn CatalogViewButton(
    lot_code: String,
    on_open: Callback<()>,
) -> Element {
    rsx! {
        button {
            class: "public-auction-catalog__icon-action",
            r#type: "button",
            aria_haspopup: "dialog",
            // 可访问名称带拍品编号，逐行可区分；悬停提示保持简短动作名。
            aria_label: t!("catalog-view-lot", code: lot_code),
            title: t!("catalog-view"),
            onclick: move |_| on_open.call(()),
            CatalogActionIcon { kind: "eye" }
        }
    }
}

/// 事件目标是否属于行本身，而不是行内自带交互的控件。
#[cfg(target_arch = "wasm32")]
fn targets_row(target: Option<web_sys::EventTarget>) -> bool {
    use wasm_bindgen::JsCast;

    target
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|element| element.closest(ROW_OR_CONTROL_SELECTOR).ok().flatten())
        .is_some_and(|nearest| nearest.tag_name().eq_ignore_ascii_case("tr"))
}

/// 拖选行内文本（例如复制装备名）同样以一次 click 结束，此时选区非空。
#[cfg(target_arch = "wasm32")]
fn selecting_text() -> bool {
    web_sys::window()
        .and_then(|window| window.get_selection().ok().flatten())
        .is_some_and(|selection| !selection.is_collapsed())
}
