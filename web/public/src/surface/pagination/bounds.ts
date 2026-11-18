// 编号页在读取前按实际页长纠偏；地址更新成功后才放行请求。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createEffect } from "solid-js";

import type { PublicBoardQuery } from "../query/board";
import { currentBrowserSearch, readPublicQuery } from "../query/location";
import { usePageRouteGuard } from "../query/route-page";
import { updatePublicQueryPairs, usePublicQuerySearch } from "./location";

/** Public 编号拍卖列表的偏移上限，等于上限的偏移仍可请求。 */
export const MAX_PUBLIC_PAGE_OFFSET = 10_000;

/** 给定接口的实际页长，计算最多可以请求的页码。 */
export function publicOffsetLastPage(size: number, offset = MAX_PUBLIC_PAGE_OFFSET): number {
  return Math.floor(offset / Math.max(size, 1)) + 1;
}

const PAGE_KEYS = {
  page: "page",
  bidPage: "bid_page",
  proxyPage: "proxy_page",
  movementPage: "movement_page",
} as const;

type PageKey = keyof typeof PAGE_KEYS;
type PageLimits = readonly (readonly [PageKey, number])[];
type Correction = readonly [PageKey, number];

function exceedsSafePage(value: string | null): boolean {
  if (value === null || !/^\+?[0-9]+$/u.test(value)) {
    return false;
  }
  const digits = value.replace(/^\+?0*/u, "");
  // 解析器把超过安全整数的有效 i64 恢复成第 1 页，须在读请求前用原值纠偏。
  if (digits.length < 16 || digits.length > 19) {
    return false;
  }
  const page = BigInt(digits);
  return page > BigInt(Number.MAX_SAFE_INTEGER) && page <= 9_223_372_036_854_775_807n;
}

/** 比较本页使用的页码，两个列表共享请求时也只改动越界的键。 */
function publicPageCorrections(
  query: PublicBoardQuery,
  search: string,
  limits: PageLimits,
  legacyBidLog: boolean,
): Correction[] {
  const params = new URLSearchParams(search);
  const changed: Correction[] = [];
  for (const [key, size] of limits) {
    const maximum = publicOffsetLastPage(size);
    const pageParam = PAGE_KEYS[key];
    const raw =
      params.getAll(pageParam).at(-1) ??
      (legacyBidLog && key === "page" ? (params.getAll("bidlog_page").at(-1) ?? null) : null);
    if (query[key] > maximum || exceedsSafePage(raw)) {
      changed.push([key, maximum]);
    }
  }
  return changed;
}

/** 只替换越界的分页参数，保留重复或非法的筛选原文供各字段修正。 */
export function usePublicPageBounds(
  state: Cell<PublicBoardQuery>,
  routeName: string,
  limits: (query: PublicBoardQuery) => PageLimits,
) {
  const routeStillOnPage = usePageRouteGuard(routeName);
  const search = usePublicQuerySearch();
  const legacyBidLog = routeName === "BidLog";
  const pending = createValueMemo(() =>
    publicPageCorrections(state(), search(), limits(state()), legacyBidLog),
  );
  const retryGeneration = createCell(0);
  const failedSearch = createCell<string | null>(null);

  createEffect(
    () => ({ changes: pending(), search: search(), retry: retryGeneration() }),
    ({ changes: pendingChanges, search: expected }) => {
      if (
        pendingChanges.length === 0 ||
        !routeStillOnPage() ||
        currentBrowserSearch() !== expected
      ) {
        return;
      }
      const current = readPublicQuery();
      const changes = publicPageCorrections(current, expected, limits(current), legacyBidLog);
      if (changes.length === 0) {
        return;
      }
      const keys = changes.map(([key]) => PAGE_KEYS[key]);
      const pairs = changes.map(([key, target]) => [PAGE_KEYS[key], String(target)] as const);
      const removeAlias = legacyBidLog && keys.includes("page");
      const written = updatePublicQueryPairs(
        state,
        removeAlias ? [...keys, "bidlog_page"] : keys,
        pairs,
        "replace",
      );
      failedSearch.set(written ? null : expected);
    },
  );

  // 两张表独立订阅各自的门禁，另一张表的纠偏不取消已经在途的合法读取。
  const allowed = {
    page: createValueMemo(() => !pending().some(([key]) => key === "page")),
    bidPage: createValueMemo(() => !pending().some(([key]) => key === "bidPage")),
    proxyPage: createValueMemo(() => !pending().some(([key]) => key === "proxyPage")),
    movementPage: createValueMemo(() => !pending().some(([key]) => key === "movementPage")),
  };
  const blocked = (key: PageKey) => !allowed[key]();
  return {
    blocked,
    failed: (key: PageKey) => blocked(key) && failedSearch() === search(),
    retry: () => retryGeneration.update((generation) => generation + 1),
  };
}
