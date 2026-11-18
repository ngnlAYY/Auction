//! 首屏目录预取：记住各世界最近一次的当前场次，下次进入看板时与 `/auctions/current` 并行读取
//! 它的 lots，省掉“先等当前场次、再请求 lots”的一次串行往返。
//!
//! 预取只是提前发出与目录完全相同的公开读取：结果经同一严格解码写入缓存，目录读取时复用在途
//! 或已写入的结果；场次已变化时目录按新场次照常读取，代价只是一次多余请求。记住的值只有场次
//! UUID（公开标识），读取时再校验格式，损坏或缺失即不预取。

use auction_web_shared::{storage, valid_uuid};
use dioxus::prelude::*;

use crate::api::{CatalogLevelFilter, prefetch_current_lots};
use crate::surface::{AuctionParameter, PublicBoardQuery};

const CURRENT_AUCTION_KEY_PREFIX: &str = "hv.public.current-auction.";

fn current_auction_key(world: &str) -> String {
    format!("{CURRENT_AUCTION_KEY_PREFIX}{world}")
}

/// 记录某世界最近一次读取到的当前场次；值不合法时不写。
pub(crate) fn remember_current_auction(world: &str, auction_id: &str) {
    if valid_uuid(auction_id)
        && storage::get_item(&current_auction_key(world)).as_deref() != Some(auction_id)
    {
        // 写入失败（隐私模式、配额）只意味着下次不预取。
        let _ = storage::set_item(&current_auction_key(world), auction_id);
    }
}

/// 看板首页启动时调用一次。只预取可按原地址读取的目录；带 `auction` 的深链不预取。
pub(crate) fn prefetch_board_lots(world: &str, query: &PublicBoardQuery) {
    if !matches!(query.auction_parameter(), AuctionParameter::Unspecified) {
        return;
    }
    let Some(category) = query.catalog_request_category() else { return };
    let Some(auction_id) = remembered_auction(world).filter(|auction_id| valid_uuid(auction_id))
    else {
        return;
    };
    let Ok(levels) = CatalogLevelFilter::parse(
        query.max_level.as_deref().unwrap_or_default(),
        &query.include_unleveled,
    ) else {
        return;
    };
    let equipment_group = query.equipment_group.clone().filter(|group| group != "all");
    spawn(prefetch_current_lots(
        auction_id,
        category.map(str::to_owned),
        equipment_group,
        levels,
        query.page,
        query.page_size,
    ));
}

fn remembered_auction(world: &str) -> Option<String> {
    let key = current_auction_key(world);
    #[cfg(test)]
    if let Some(result) = crate::surface::PageReadFixture::response(key.clone()) {
        return result;
    }
    storage::get_item(&key)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::prefetch_board_lots;
    use crate::api::LotEnvelope;
    use crate::surface::query::parse_public_query;
    use crate::surface::{PageReadFixture, settle_page};

    const AUCTION: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";

    #[test]
    fn boot_prefetch_only_reads_a_valid_category_and_offset() {
        for (search, expected) in [
            ("?type=unknown", None),
            ("?type=clo&type=shd", None),
            ("?type=clo&page=52&page_size=200", None),
            ("?type=clo&page=9223372036854775807&page_size=200", None),
            ("?type=clo&page=51&page_size=200", Some("page=51&page_size=200&category=clo")),
        ] {
            let requests = Rc::new(RefCell::new(Vec::new()));
            let gets = requests.clone();
            let mut dom = VirtualDom::new(|| rsx! {})
                .with_root_context(PageReadFixture::new(|key: String| {
                    assert_eq!(key, "hv.public.current-auction.main");
                    Some(AUCTION.to_owned())
                }))
                .with_root_context(PageReadFixture::new(move |url: String| {
                    gets.borrow_mut().push(url);
                    Err::<LotEnvelope, _>("fixture".to_owned())
                }));
            dom.rebuild_in_place();
            dom.in_scope(ScopeId::APP, || {
                prefetch_board_lots("main", &parse_public_query("/", search));
            });
            settle_page(&mut dom);
            let expected = expected
                .map(|query| format!("/api/v1/auctions/{AUCTION}/lots?{query}&fields=summary"));
            assert_eq!(*requests.borrow(), expected.into_iter().collect::<Vec<_>>());
        }
    }
}
