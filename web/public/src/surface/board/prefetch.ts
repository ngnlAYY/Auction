// 首屏目录预取：记住各世界最近一次的当前场次，下次进入看板时与 `/auctions/current` 并行读取
// 它的 lots，省掉“先等当前场次、再请求 lots”的一次串行往返。
//
// 预取只是提前发出与目录完全相同的公开读取：结果经同一严格解码写入缓存，目录读取时复用在途
// 或已写入的结果；场次已变化时目录按新场次照常读取，代价只是一次多余请求。记住的值只有场次
// UUID（公开标识），读取时再校验格式，损坏或缺失即不预取。

import { getItem, setItem } from "@auction/web-shared/platform/storage";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { validUuid } from "@auction/web-shared/ui/validation";

import { catalogLevelFilterParse } from "../../api/model";
import { prefetchCurrentLots } from "../../api/queries/lots";
import type { PublicBoardQuery } from "../query";

const CURRENT_AUCTION_KEY_PREFIX = "hv.public.current-auction.";

function currentAuctionKey(world: string): string {
  return `${CURRENT_AUCTION_KEY_PREFIX}${world}`;
}

/** 记录某世界最近一次读取到的当前场次；值不合法时不写。 */
export function rememberCurrentAuction(world: string, auctionId: string): void {
  if (validUuid(auctionId) && getItem(currentAuctionKey(world)) !== auctionId) {
    // 写入失败（隐私模式、配额）只意味着下次不预取。
    setItem(currentAuctionKey(world), auctionId);
  }
}

/** 看板首页启动时调用一次。参数推导与看板目录读取完全一致（分类/分组 `all` 视为不筛选、页码
 * 至少为 1），保证两者请求同一 URL；带 `auction` 深链的页面读取历史场次，不预取。 */
export function prefetchBoardLots(world: string, query: PublicBoardQuery): void {
  if (query.auctionId !== null) {
    return;
  }
  const stored = getItem(currentAuctionKey(world));
  if (stored === null || !validUuid(stored)) {
    return;
  }
  const levels = catalogLevelFilterParse(query.maxLevel ?? "", query.includeUnleveled);
  if (!levels.ok) {
    return;
  }
  const category = query.category === "all" ? null : query.category;
  const equipmentGroup = query.equipmentGroup === "all" ? null : query.equipmentGroup;
  const tasks = createTaskScope();
  tasks.spawn(async (context) => {
    await prefetchCurrentLots(
      stored,
      category,
      equipmentGroup,
      levels.value,
      Math.max(query.page, 1),
      query.pageSize,
      context.signal,
    );
  });
}
