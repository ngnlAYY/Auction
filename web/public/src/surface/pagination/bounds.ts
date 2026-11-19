// 编号页在读取前按实际页长纠偏；地址更新成功后才放行请求。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createEffect } from "solid-js";

import type { PublicBoardQuery } from "../query/board";
import { currentBrowserSearch, publicQueryPage, readPublicQueryForState } from "../query/location";
import { usePageRouteGuard } from "../query/route-page";
import { updatePublicQueryPairs, usePublicQuerySearch } from "./location";
import { PAGE_KEYS, publicOffsetLastPage, readPublicPageParameters } from "./parameters";

export { MAX_PUBLIC_PAGE_OFFSET, publicOffsetLastPage } from "./parameters";

type PageKey = keyof typeof PAGE_KEYS;
type PageLimits = readonly (readonly [PageKey, number])[];
type Correction = readonly [PageKey, number];

/** 比较本页使用的页码，两个列表共享请求时也只改动越界的键。 */
export function publicPageCorrections(
  search: string,
  limits: PageLimits,
  legacyBidLog: boolean,
): Correction[] {
  const pages = readPublicPageParameters(new URLSearchParams(search), legacyBidLog);
  const changed: Correction[] = [];
  for (const [key, size] of limits) {
    const maximum = publicOffsetLastPage(size);
    const page = pages[key];
    if (page !== null && page > BigInt(maximum)) {
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
  const pending = createValueMemo(() =>
    publicPageCorrections(search(), limits(state()), publicQueryPage(state) === "BidLog"),
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
      const current = readPublicQueryForState(state);
      const legacyBidLog = publicQueryPage(state) === "BidLog";
      const changes = publicPageCorrections(expected, limits(current), legacyBidLog);
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
