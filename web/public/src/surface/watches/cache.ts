//! 非敏感装备标记缓存；按稳定用户和拍卖 UUID 隔离，不保存凭据、装备链接或代理上限。

import type { PublicLotWatchesResponse } from "@auction/contracts/dto/public/watch";
import { PublicLotWatchesResponseSchema } from "@auction/contracts/dto/public/watch";
import { decode } from "@auction/contracts/wire";
import {
  getItem,
  getParsed,
  keys,
  removeItem,
  setItem,
} from "@auction/web-shared/platform/storage";

import { object } from "../../api/decoder";
import { publicWatchCacheRevision } from "../../api/session/revision";
import { validWatchCollection } from "../../api/watches";

/** 条目键前缀；索引键使用不同分隔符，按前缀淘汰时不会命中索引本身。 */
const KEY_PREFIX = "hv.public.lot-watches.v1:";
const INDEX_KEY = "hv.public.lot-watches.v1#index";
const CACHE_VERSION = 2;
/** 只保留最近访问的三场拍卖，避免长期使用后无限占用本地配额。 */
const MAX_AUCTIONS = 3;

/** 缓存只能展示，不能提供修改命令的版本，也不能上传覆盖服务端。 */
export function load(userId: string, auctionId: string): PublicLotWatchesResponse | null {
  const revision = publicWatchCacheRevision();
  if (revision === null) {
    return null;
  }
  const page = getParsed(key(userId, auctionId), (value) => {
    const entry = object(value);
    if (entry?.version !== CACHE_VERSION || entry.session_revision !== revision) {
      return null;
    }
    const decoded = decode(PublicLotWatchesResponseSchema, value);
    return decoded.ok ? decoded.value : null;
  });
  if (page === null || revision !== publicWatchCacheRevision()) {
    return null;
  }
  return validWatchCollection(page, userId, auctionId) ? page : null;
}

/** 存储禁用、配额不足等错误不改变服务器保存结果。淘汰只发生在本缓存自己的前缀内，
 * 语言、主题与每页数量偏好不会被挤掉；写入失败则不记录索引也不删除任何键。 */
export function save(page: PublicLotWatchesResponse, revision: string | null): void {
  if (revision === null || revision !== publicWatchCacheRevision()) {
    return;
  }
  // 写入期间其他标签页仍能切换身份；读取时必须按请求发起的代次拒绝旧快照。
  const value = JSON.stringify({ version: CACHE_VERSION, session_revision: revision, ...page });
  const current = key(page.user_id, page.auction_id);
  if (!setItem(current, value)) {
    return;
  }
  let recent = recentKeys();
  recent = recent.filter((entry) => entry !== current);
  recent.unshift(current);
  recent = recent.slice(0, MAX_AUCTIONS);
  if (!setItem(INDEX_KEY, recent.join("\n"))) {
    return;
  }
  for (const stale of keys()) {
    if (stale.startsWith(KEY_PREFIX) && !recent.includes(stale)) {
      removeItem(stale);
    }
  }
}

/** 已确认身份离开时丢弃该用户各场快照，避免重新登录沿用上一段会话的标记。 */
export function invalidateLotWatches(userId: string): void {
  const prefix = `${KEY_PREFIX}${userId}:`;
  // 直接枚举条目，索引缺失或不完整时也能丢弃旧快照。
  for (const entry of keys()) {
    if (entry.startsWith(prefix)) {
      removeItem(entry);
    }
  }
  const recent = recentKeys();
  const retained = recent.filter((entry) => !entry.startsWith(prefix));
  if (retained.length !== recent.length) {
    setItem(INDEX_KEY, retained.join("\n"));
  }
}

/** 读取最近保存的条目键；索引缺失、损坏或来自旧版本时按空索引处理。 */
function recentKeys(): string[] {
  const index = getItem(INDEX_KEY);
  if (index === null) {
    return [];
  }
  return index.split("\n").filter((entry) => entry.startsWith(KEY_PREFIX));
}

function key(userId: string, auctionId: string): string {
  return `${KEY_PREFIX}${userId}:${auctionId}`;
}
