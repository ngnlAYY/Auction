// Public 页码的 URL 取值与编号分页的偏移上限。

const INT64_MAX = 9_223_372_036_854_775_807n;
const SAFE_PAGE_MAX = BigInt(Number.MAX_SAFE_INTEGER);

/** Public 编号拍卖列表的偏移上限，等于上限的偏移仍可请求。 */
export const MAX_PUBLIC_PAGE_OFFSET = 10_000;

/** 给定接口的实际页长，计算最多可以请求的页码。 */
export function publicOffsetLastPage(size: number, offset = MAX_PUBLIC_PAGE_OFFSET): number {
  return Math.floor(offset / Math.max(size, 1)) + 1;
}

/** 查询字段及对应的正 i64 页码参数。 */
export const PAGE_KEYS = {
  page: "page",
  bidPage: "bid_page",
  proxyPage: "proxy_page",
  movementPage: "movement_page",
} as const;

function lastPositivePage(params: URLSearchParams, key: string, maximum: bigint): bigint | null {
  const values = params.getAll(key);
  for (let index = values.length - 1; index >= 0; index -= 1) {
    const value = values[index] ?? "";
    if (!/^\+?[0-9]+$/u.test(value)) continue;
    // 前导零不计入位宽，任意长的非法整数也不交给 BigInt。
    const digits = value.replace(/^\+?0*/u, "");
    if (digits.length === 0 || digits.length > 19) continue;
    const page = BigInt(digits);
    if (page <= maximum) return page;
  }
  return null;
}

/** 按 Rust 的优先规则读取页码；合法的超大值仍参与重复键与别名的选择。 */
export function readPublicPageParameters(
  params: URLSearchParams,
  legacyBidLog: boolean,
  safeIntegerOnly = false,
) {
  const maximum = safeIntegerOnly ? SAFE_PAGE_MAX : INT64_MAX;
  const alias = legacyBidLog ? lastPositivePage(params, "bidlog_page", maximum) : null;
  return {
    page: lastPositivePage(params, PAGE_KEYS.page, maximum) ?? alias,
    bidPage: lastPositivePage(params, PAGE_KEYS.bidPage, maximum),
    proxyPage: lastPositivePage(params, PAGE_KEYS.proxyPage, maximum),
    movementPage: lastPositivePage(params, PAGE_KEYS.movementPage, maximum),
    bidLogPageAlias: alias !== null,
  };
}
