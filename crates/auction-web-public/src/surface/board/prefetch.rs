//! 首屏目录预取：记住各世界最近一次的当前场次，下次进入看板时与 `/auctions/current` 并行读取
//! 它的 lots，省掉“先等当前场次、再请求 lots”的一次串行往返。
//!
//! 预取只是提前发出与目录完全相同的公开读取：结果经同一严格解码写入缓存，目录读取时复用在途
//! 或已写入的结果；场次已变化时目录按新场次照常读取，代价只是一次多余请求。记住的值只有场次
//! UUID（公开标识），读取时再校验格式，损坏或缺失即不预取。

use auction_web_shared::{storage, valid_uuid};
use dioxus::prelude::*;

use crate::api::{CatalogLevelFilter, prefetch_current_lots};
use crate::surface::PublicBoardQuery;

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

/// 看板首页启动时调用一次。参数推导与看板目录读取完全一致（分类/分组 `all` 视为不筛选、页码
/// 至少为 1），保证两者请求同一 URL；带 `auction` 深链的页面读取历史场次，不预取。
pub(crate) fn prefetch_board_lots(world: &str, query: &PublicBoardQuery) {
    if query.auction_id.is_some() {
        return;
    }
    let Some(auction_id) =
        storage::get_item(&current_auction_key(world)).filter(|auction_id| valid_uuid(auction_id))
    else {
        return;
    };
    let Ok(levels) = CatalogLevelFilter::parse(
        query.max_level.as_deref().unwrap_or_default(),
        &query.include_unleveled,
    ) else {
        return;
    };
    let category = query.category.clone().filter(|category| category != "all");
    let equipment_group = query.equipment_group.clone().filter(|group| group != "all");
    spawn(prefetch_current_lots(
        auction_id,
        category,
        equipment_group,
        levels,
        query.page.max(1),
        query.page_size,
    ));
}
