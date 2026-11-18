//! 拍品直达链接：被超提醒等站外入口用 `/lots/{拍品 UUID}` 指向一件拍品。
//! 独立的拍品页已移除，这里把链接解析成目录位置（世界、场次、分类与页码），跳到看板后打开
//! 该拍品的详情抽屉。请求只是一次性的展示意图：不写 URL、不创建命令，也不判定出价资格——
//! 目录里有这一行时由行自己打开抽屉（出价入口照常由行内操作栏投影），找不到时退回只读抽屉。

use std::rc::Rc;

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::lookup::LotDetailLookup;
use crate::api::{
    CatalogRow, CurrentAuction, catalog_row_from_detail, fetch_current_auction,
    fetch_historical_auction, fetch_lot_detail,
};
use crate::surface::{
    LotLinkLocation, LotLinkRequest, PublicBoardQuery, catalog_category_key, normalize_world_query,
    public_href, use_content_loading,
};

/// 带已读详情的一次性请求；只有看板确认对齐行存在后才允许行消费。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LotViewTarget {
    pub lot_id: String,
    pub auction_id: String,
    pub world: Option<String>,
    pub row: Rc<CatalogRow>,
    pub row_ready: bool,
}

/// 解析页与看板共用的一次性抽屉意图。
#[derive(Clone, Copy)]
pub(crate) struct LotViewRequest(pub Signal<Option<LotViewTarget>>);

impl LotViewRequest {
    pub(crate) fn deliver(
        mut self,
        mut target: LotViewTarget,
        disposition: LotViewDisposition,
        lookup: LotDetailLookup,
    ) {
        if self.0.peek().as_ref() != Some(&target) {
            return;
        }
        match disposition {
            LotViewDisposition::Wait => {}
            LotViewDisposition::Row if !target.row_ready => {
                target.row_ready = true;
                self.0.set(Some(target));
            }
            LotViewDisposition::Row => {}
            LotViewDisposition::ReadOnly => {
                self.0.set(None);
                lookup.view(target);
            }
        }
    }
}

/// 看板对直达请求的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LotViewDisposition {
    Wait,
    Row,
    ReadOnly,
}

/// 只有当前读取尚未结束才等待；空场、错误或当前页无目标时复用已读详情。
pub(crate) fn dispose_lot_view<'a>(
    target: &LotViewTarget,
    url_world: &str,
    displayed: Option<(&str, &str)>,
    reading: bool,
    page_lots: impl IntoIterator<Item = &'a str>,
) -> LotViewDisposition {
    let Some(world) = target.world.as_deref() else {
        return LotViewDisposition::ReadOnly;
    };
    if world != url_world || displayed.is_some_and(|(world, _)| world != url_world) {
        return if reading { LotViewDisposition::Wait } else { LotViewDisposition::ReadOnly };
    }
    if displayed.is_some_and(|(_, auction_id)| target.auction_id != auction_id) {
        return LotViewDisposition::ReadOnly;
    }
    if page_lots.into_iter().any(|lot_id| lot_id == target.lot_id) {
        return LotViewDisposition::Row;
    }
    if reading { LotViewDisposition::Wait } else { LotViewDisposition::ReadOnly }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LotLinkError {
    ReadFailed,
    Missing,
}

/// 失败时保留解析页与冻结位置；成功后用原有首页 URL 和一次性抽屉请求完成导航。
#[component]
pub(crate) fn LotDeepLink(lot_id: String) -> Element {
    let mut request = use_context::<LotViewRequest>().0;
    let mut location = use_context::<LotLinkRequest>().0;
    let mut details = use_resource(use_reactive((&lot_id,), move |(lot_id,)| {
        let known = location.read().as_ref().filter(|target| target.lot_id == lot_id).cloned();
        async move {
            let result = resolve(lot_id.clone(), known).await;
            (lot_id, result)
        }
    }));
    let loading = *details.state().read() == UseResourceState::Pending;
    use_content_loading(loading);
    use_effect(use_reactive((&lot_id,), move |(lot_id,)| {
        let loaded = details.read();
        if let Some((resolved, Ok((target, query)))) = loaded.as_ref()
            && resolved == &lot_id
            && *details.state().read() == UseResourceState::Ready
        {
            request.set(Some(target.clone()));
            let _ = navigator().replace(public_href("/", query));
        }
    }));
    let leaving_lot = lot_id.clone();
    use_drop(move || {
        // 成功跳转和中途离开共用这条清理路径，切换拍品时保留新作用域的位置。
        let owns_location = location.try_peek().is_ok_and(|current| {
            current.as_ref().is_some_and(|target| target.lot_id == leaving_lot)
        });
        if owns_location {
            location.set(None);
        }
    });
    let failure = details.read().as_ref().and_then(|(resolved, result)| {
        (resolved == &lot_id && !loading).then(|| result.as_ref().err().copied()).flatten()
    });
    let retry = use_callback(move |()| details.restart());
    #[cfg(test)]
    tests::observe_retry((failure == Some(LotLinkError::ReadFailed)).then_some(retry));
    match failure {
        Some(LotLinkError::ReadFailed) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("catalog-detail"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| retry.call(()),
            }
        },
        Some(LotLinkError::Missing) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("catalog-detail"),
                detail: t!("catalog-detail-not-found"),
            }
        },
        None => rsx! {},
    }
}

async fn read_link_lot(lot_id: String) -> Result<(String, Rc<CatalogRow>), LotLinkError> {
    #[cfg(test)]
    if let Some(result) = tests::lot_response(&lot_id) {
        return result;
    }
    let page = fetch_lot_detail(lot_id).await.map_err(|error| {
        if error.not_found() { LotLinkError::Missing } else { LotLinkError::ReadFailed }
    })?;
    let row = page.lot.map(catalog_row_from_detail).ok_or(LotLinkError::ReadFailed)?;
    Ok((page.auction_id, Rc::new(row)))
}

async fn resolve(
    lot_id: String,
    known: Option<LotLinkLocation>,
) -> Result<(LotViewTarget, PublicBoardQuery), LotLinkError> {
    // 冻结目标走指定场次，即使当前场次已轮换也无需重新猜所在世界。
    if let Some(known) = known {
        let (auction_id, row) = read_link_lot(lot_id.clone()).await?;
        if auction_id != known.auction_id {
            return Err(LotLinkError::ReadFailed);
        }
        let query = catalog_position(
            &row,
            &known.world,
            Some(auction_id.clone()),
            PublicBoardQuery::default(),
        );
        return Ok((view_target(lot_id, auction_id, Some(known.world), row), query));
    }
    let (lot, main, isekai) = futures_util::join!(
        read_link_lot(lot_id.clone()),
        read_current("main"),
        read_current("isekai"),
    );
    let (auction_id, row) = lot?;
    let (world, query) = if let Some(world) = current_world(&auction_id, [main, isekai]) {
        let query = catalog_position(&row, &world, None, PublicBoardQuery::default());
        (Some(world), query)
    } else if let Ok(auction) = read_historical(auction_id.clone()).await {
        let query = catalog_position(
            &row,
            &auction.world,
            Some(auction_id.clone()),
            PublicBoardQuery::default(),
        );
        (Some(auction.world), query)
    } else {
        // 定位失败时仍带已读详情交给首页的只读宿主。
        (None, PublicBoardQuery::default())
    };
    Ok((view_target(lot_id, auction_id, world, row), query))
}

fn view_target(
    lot_id: String,
    auction_id: String,
    world: Option<String>,
    row: Rc<CatalogRow>,
) -> LotViewTarget {
    LotViewTarget { lot_id, auction_id, world, row, row_ready: false }
}

async fn read_current(world: &str) -> Result<Option<CurrentAuction>, String> {
    #[cfg(test)]
    if tests::observe_location(world) {
        return Ok(None);
    }
    fetch_current_auction(world.to_owned()).await
}

async fn read_historical(auction_id: String) -> Result<CurrentAuction, String> {
    #[cfg(test)]
    if tests::observe_location(&auction_id) {
        return Err("network_error".to_owned());
    }
    fetch_historical_auction(auction_id).await
}

/// 拍品所在场次是哪个世界的当前场次。读取失败的世界不参与判定；都不命中（含读取失败）时
/// 返回 `None`，由调用方按往期场次查找。
fn current_world(
    auction_id: &str,
    currents: impl IntoIterator<Item = Result<Option<CurrentAuction>, String>>,
) -> Option<String> {
    currents
        .into_iter()
        .filter_map(|current| current.ok().flatten())
        .find(|current| current.id == auction_id)
        .map(|current| current.world)
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
pub(in crate::surface::catalog) mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::prelude::EventHandler;

    use super::{
        CatalogRow, CurrentAuction, LotLinkError, LotViewDisposition, LotViewTarget,
        PublicBoardQuery, catalog_page, catalog_position, current_world, dispose_lot_view,
    };

    struct Script {
        lot: Option<(String, Rc<CatalogRow>)>,
        failures: usize,
        locations: Vec<String>,
    }

    thread_local! {
        static SCRIPT: RefCell<Option<Script>> = const { RefCell::new(None) };
        pub(in crate::surface::catalog) static RETRY: Cell<Option<EventHandler<()>>> = const { Cell::new(None) };
    }

    pub(in crate::surface::catalog) fn reset() {
        SCRIPT.set(None);
        RETRY.set(None);
    }

    pub(in crate::surface::catalog) fn script(lot_id: &str, auction_id: &str, failures: usize) {
        let mut row = row("S07", "Shield");
        row.lot_id = lot_id.to_owned();
        row.name = "Authority Shield".to_owned();
        row.status = "pending".to_owned();
        SCRIPT.set(Some(Script {
            lot: Some((auction_id.to_owned(), Rc::new(row))),
            failures,
            locations: Vec::new(),
        }));
    }

    pub(in crate::surface::catalog) fn use_wire_response() {
        SCRIPT.set(Some(Script { lot: None, failures: 0, locations: Vec::new() }));
    }

    pub(in crate::surface::catalog) fn location_lookups() -> Vec<String> {
        SCRIPT.with_borrow(|state| state.as_ref().unwrap().locations.clone())
    }

    pub(super) fn observe_retry(retry: Option<EventHandler<()>>) {
        RETRY.set(retry);
    }

    pub(in crate::surface::catalog) fn scripted() -> bool {
        SCRIPT.with_borrow(Option::is_some)
    }

    pub(super) fn lot_response(
        lot_id: &str,
    ) -> Option<Result<(String, Rc<CatalogRow>), LotLinkError>> {
        SCRIPT.with_borrow_mut(|state| {
            let state = state.as_mut()?;
            let lot = state.lot.as_ref()?;
            assert_eq!(lot.1.lot_id, lot_id);
            if state.failures > 0 {
                state.failures -= 1;
                return Some(Err(LotLinkError::ReadFailed));
            }
            Some(Ok(lot.clone()))
        })
    }

    pub(super) fn observe_location(location: &str) -> bool {
        SCRIPT.with_borrow_mut(|state| {
            if let Some(state) = state {
                state.locations.push(location.to_owned());
                true
            } else {
                false
            }
        })
    }

    fn target() -> LotViewTarget {
        LotViewTarget {
            lot_id: "lot-16".to_owned(),
            auction_id: "auction-7".to_owned(),
            world: Some("main".to_owned()),
            row: Rc::new(row("Clo16", "Cloth")),
            row_ready: false,
        }
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

    fn current(auction_id: &str, world: &str) -> Result<Option<CurrentAuction>, String> {
        Ok(Some(CurrentAuction {
            id: auction_id.to_owned(),
            world: world.to_owned(),
            number: 12,
            status: "live".to_owned(),
            opens_at: "2026-11-01T00:00:00Z".to_owned(),
            scheduled_ends_at: Some("2026-11-08T00:00:00Z".to_owned()),
            presentation_mode: "live".to_owned(),
            bidding_open: true,
            preview_revision: 1,
            proxy_default_mode: Some("fast".to_owned()),
        }))
    }

    #[test]
    fn current_world_names_the_world_whose_current_auction_matches() {
        assert_eq!(
            current_world(
                "auction-7",
                [current("auction-7", "main"), current("auction-9", "isekai")]
            ),
            Some("main".to_owned())
        );
        assert_eq!(
            current_world(
                "auction-9",
                [current("auction-7", "main"), current("auction-9", "isekai")]
            ),
            Some("isekai".to_owned())
        );
    }

    #[test]
    fn unmatched_or_unreadable_currents_fall_back_to_the_historical_lookup() {
        // 往期场次：两个世界的当前场次都不是它。
        assert_eq!(
            current_world(
                "auction-3",
                [current("auction-7", "main"), current("auction-9", "isekai")]
            ),
            None
        );
        // 世界暂无当前场次。
        assert_eq!(current_world("auction-7", [Ok(None), Ok(None)]), None);
        // 所在世界读取失败时不下结论，交给往期查找；另一世界照常参与匹配。
        assert_eq!(
            current_world("auction-7", [Err("network".to_owned()), current("auction-9", "isekai")]),
            None
        );
        assert_eq!(
            current_world("auction-9", [Err("network".to_owned()), current("auction-9", "isekai")]),
            Some("isekai".to_owned())
        );
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
        assert_eq!(
            dispose_lot_view(&target(), "isekai", Some(("main", "auction-1")), true, ["lot-1"]),
            LotViewDisposition::Wait
        );
        assert_eq!(
            dispose_lot_view(&target(), "main", Some(("main", "auction-7")), true, []),
            LotViewDisposition::Wait
        );
    }

    #[test]
    fn row_opens_the_drawer_when_it_is_on_the_page() {
        assert_eq!(
            dispose_lot_view(
                &target(),
                "main",
                Some(("main", "auction-7")),
                false,
                ["lot-15", "lot-16"]
            ),
            LotViewDisposition::Row
        );
    }

    #[test]
    fn falls_back_after_the_board_or_catalogue_finishes_without_the_row() {
        for (displayed, lots) in
            [(None, []), (Some(("main", "auction-7")), []), (Some(("main", "auction-8")), [])]
        {
            assert_eq!(
                dispose_lot_view(&target(), "main", displayed, false, lots),
                LotViewDisposition::ReadOnly
            );
        }
        let mut target = target();
        target.world = None;
        assert_eq!(
            dispose_lot_view(&target, "main", None, true, []),
            LotViewDisposition::ReadOnly,
            "场次定位失败时可以直接使用已读详情"
        );
    }
}
