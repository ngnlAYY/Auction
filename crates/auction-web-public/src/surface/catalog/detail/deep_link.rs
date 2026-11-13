//! 拍品直达链接：被超提醒等站外入口用 `/lots/{拍品 UUID}` 指向一件拍品。
//! 独立的拍品页已移除，这里把链接解析成目录位置（世界、场次、分类与页码），跳到看板后打开
//! 该拍品的详情抽屉。请求只是一次性的展示意图：不写 URL、不创建命令，也不判定出价资格——
//! 目录里有这一行时由行自己打开抽屉（出价入口照常由行内操作栏投影），找不到时退回只读抽屉。

use dioxus::prelude::*;

use crate::api::{
    CatalogRow, catalog_row_from_detail, fetch_current_auction, fetch_historical_auction,
    fetch_lot_detail,
};
use crate::surface::{
    PublicBoardQuery, catalog_category_key, normalize_world_query, public_href, use_content_loading,
};

/// 一次直达请求要查看的拍品。`auction_id` 来自拍品响应本身，是看板判断“当前展示的是不是
/// 这件拍品所在场次”的依据。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LotViewTarget {
    pub lot_id: String,
    pub auction_id: String,
}

/// 外壳持有的直达请求：解析页写入，看板处置一次后清空。它跨路由存活，但不进入 URL，
/// 刷新或后退都不会重新弹出抽屉。
#[derive(Clone, Copy)]
pub(crate) struct LotViewRequest(pub Signal<Option<LotViewTarget>>);

/// 看板对直达请求的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LotViewDisposition {
    /// 看板还没落到目标位置（世界仍在同步，或目录页尚未读出）：保留请求。
    Wait,
    /// 目录当前页有这一行：交给行自己打开抽屉，出价与标记入口照常可用。
    Row,
    /// 看板展示的不是这件拍品所在的场次，或这一页没有它：退回只读抽屉。
    ReadOnly,
}

/// 判定当前看板如何处置直达请求。
///
/// `url_world` 与 `displayed_world` 不一致说明外壳的世界选择还没跟上地址栏，此时展示的是
/// 上一个世界的场次，不能据此下结论。`page_lots` 是已与当前筛选、页码对齐的目录页；
/// 读出之前一律等待。
pub(crate) fn dispose_lot_view<'a>(
    target: &LotViewTarget,
    url_world: &str,
    displayed_world: &str,
    displayed_auction_id: &str,
    page_lots: Option<impl IntoIterator<Item = &'a str>>,
) -> LotViewDisposition {
    if url_world != displayed_world {
        return LotViewDisposition::Wait;
    }
    if target.auction_id != displayed_auction_id {
        return LotViewDisposition::ReadOnly;
    }
    match page_lots {
        None => LotViewDisposition::Wait,
        Some(lots) => {
            if lots.into_iter().any(|lot_id| lot_id == target.lot_id) {
                LotViewDisposition::Row
            } else {
                LotViewDisposition::ReadOnly
            }
        }
    }
}

/// `/lots/{拍品 UUID}` 的解析页：读出拍品所在位置后无历史记录地跳到看板。
/// 读取失败（拍品不存在、网络错误）同样回到看板，只是不带直达请求。
#[component]
pub(crate) fn LotDeepLink(lot_id: String) -> Element {
    let mut request = use_context::<LotViewRequest>().0;
    // 解析期间正文为空，加载提示交给顶栏。
    use_content_loading(true);
    // 以拍品为依赖：同一组件实例换了参数时重新解析，而不是沿用上一件的结果。
    use_effect(use_reactive((&lot_id,), move |(lot_id,)| {
        spawn(async move {
            let href = match resolve(lot_id).await {
                Some((target, query)) => {
                    request.set(Some(target));
                    public_href("/", &query)
                }
                None => "/".to_owned(),
            };
            let _ = navigator().replace(href);
        });
    }));
    rsx! {}
}

/// 读出拍品及其场次，算出看板应当落到的目录位置。
async fn resolve(lot_id: String) -> Option<(LotViewTarget, PublicBoardQuery)> {
    let page = fetch_lot_detail(lot_id.clone()).await.ok()?;
    let auction_id = page.auction_id.clone();
    let row = page.lot.map(catalog_row_from_detail)?;
    let target = LotViewTarget { lot_id, auction_id: auction_id.clone() };
    // 拍品响应不带世界。读不出场次时仍回到看板并保留请求：看板展示的不会是这个场次，
    // 于是按只读抽屉打开，链接不至于落空。
    let Ok(auction) = fetch_historical_auction(auction_id.clone()).await else {
        return Some((target, PublicBoardQuery::default()));
    };
    // 进行中的场次落到该世界的首页（不带场次参数），与用户平时看到的看板一致；读取失败按
    // 往期场次处理，指定场次的看板同样可以出价。
    let is_current = fetch_current_auction(auction.world.clone())
        .await
        .ok()
        .flatten()
        .is_some_and(|current| current.id == auction_id);
    let query = catalog_position(
        &row,
        &auction.world,
        (!is_current).then_some(auction_id),
        PublicBoardQuery::default(),
    );
    Some((target, query))
}

/// 拍品在目录里的位置：所在世界、（往期）场次、分类筛选与页码。其余筛选保持默认，
/// 每页条数沿用 `base` 里的本地偏好。
fn catalog_position(
    row: &CatalogRow,
    world: &str,
    historical_auction_id: Option<String>,
    base: PublicBoardQuery,
) -> PublicBoardQuery {
    let category = catalog_category_key(&row.category);
    // “其他”不是可筛选的分类：不带分类时目录按全场序号排列，分类内序号推不出页码。
    let (category, page) = if category == "other" {
        (None, 1)
    } else {
        (Some(category.to_owned()), catalog_page(&row.code, base.page_size).unwrap_or(1))
    };
    PublicBoardQuery {
        // 主世界沿用无参数 URL。
        world: normalize_world_query(world).filter(|world| world != "main"),
        auction_id: historical_auction_id,
        category,
        page,
        ..base
    }
}

/// 按分类筛选后拍品所在的页码。编号末尾的数字是分类内序号，目录按同一顺序排列，第 n 件在
/// 第 ⌈n / 每页条数⌉ 页。撤拍留下的空号会让实际位置靠前，那时看板找不到这一行，退回只读抽屉。
fn catalog_page(code: &str, page_size: i64) -> Option<i64> {
    let digits = code.trim_start_matches(|character: char| !character.is_ascii_digit());
    let ordinal = digits.parse::<i64>().ok().filter(|ordinal| *ordinal > 0)?;
    (page_size > 0).then(|| (ordinal - 1) / page_size + 1)
}

#[cfg(test)]
mod tests {
    use super::{
        CatalogRow, LotViewDisposition, LotViewTarget, PublicBoardQuery, catalog_page,
        catalog_position, dispose_lot_view,
    };

    fn target() -> LotViewTarget {
        LotViewTarget { lot_id: "lot-16".to_owned(), auction_id: "auction-7".to_owned() }
    }

    fn row(code: &str, category: &str) -> CatalogRow {
        CatalogRow {
            lot_id: "lot-16".to_owned(),
            code: code.to_owned(),
            name: "Phase Robe of Niflheim".to_owned(),
            game_equipment_url: String::new(),
            category: category.to_owned(),
            equipment_group: String::new(),
            status: "live".to_owned(),
            seller_name: "seller".to_owned(),
            level: None,
            primary_attribute_initials: None,
            avg_roll: None,
            start_price: 1_000,
            current_price: 1_000,
            next_bid: 2_000,
            leader_name: None,
            remaining: String::new(),
        }
    }

    fn base(page_size: i64) -> PublicBoardQuery {
        PublicBoardQuery { page_size, ..PublicBoardQuery::default() }
    }

    #[test]
    fn page_follows_the_ordinal_inside_the_category() {
        assert_eq!(catalog_page("Clo01", 10), Some(1));
        assert_eq!(catalog_page("Clo10", 10), Some(1));
        assert_eq!(catalog_page("Clo11", 10), Some(2));
        assert_eq!(catalog_page("Hea120", 50), Some(3));
        // 没有序号或序号非法时不猜页码。
        assert_eq!(catalog_page("Clo", 10), None);
        assert_eq!(catalog_page("Clo00", 10), None);
        assert_eq!(catalog_page("Clo16", 0), None);
    }

    #[test]
    fn current_auction_lands_on_the_world_home_with_category_and_page() {
        let query = catalog_position(&row("Clo16", "Cloth"), "main", None, base(10));
        assert_eq!(
            (query.world, query.auction_id, query.category, query.page, query.page_size),
            (None, None, Some("clo".to_owned()), 2, 10)
        );
        // 异世界带世界参数；往期场次带场次参数。
        let query = catalog_position(
            &row("Hea03", "Heavy"),
            "isekai",
            Some("auction-7".to_owned()),
            base(20),
        );
        assert_eq!(
            (query.world, query.auction_id, query.category, query.page),
            (Some("isekai".to_owned()), Some("auction-7".to_owned()), Some("hea".to_owned()), 1)
        );
    }

    #[test]
    fn unfilterable_category_keeps_the_first_unfiltered_page() {
        let query = catalog_position(&row("Lot07", "Accessory"), "main", None, base(10));
        assert_eq!((query.category, query.page), (None, 1));
    }

    #[test]
    fn request_waits_until_the_board_shows_the_target_world_and_page() {
        // 外壳的世界选择还没跟上地址栏：展示的是另一个世界的场次，不下结论。
        assert_eq!(
            dispose_lot_view(&target(), "isekai", "main", "auction-1", Some(["lot-1"])),
            LotViewDisposition::Wait
        );
        // 目录页尚未读出。
        assert_eq!(
            dispose_lot_view(&target(), "main", "main", "auction-7", None::<[&str; 0]>),
            LotViewDisposition::Wait
        );
    }

    #[test]
    fn row_opens_the_drawer_when_it_is_on_the_page() {
        assert_eq!(
            dispose_lot_view(&target(), "main", "main", "auction-7", Some(["lot-15", "lot-16"])),
            LotViewDisposition::Row
        );
    }

    #[test]
    fn falls_back_to_the_read_only_drawer_when_the_row_is_not_there() {
        // 页码因撤拍空号偏后：这一页没有它。
        assert_eq!(
            dispose_lot_view(&target(), "main", "main", "auction-7", Some(["lot-21"])),
            LotViewDisposition::ReadOnly
        );
        // 场次已经轮换：看板展示的是同一世界的另一场，不必等目录读完。
        assert_eq!(
            dispose_lot_view(&target(), "main", "main", "auction-8", None::<[&str; 0]>),
            LotViewDisposition::ReadOnly
        );
    }
}
