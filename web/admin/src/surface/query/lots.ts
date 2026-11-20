// lot 列表的 query 状态：页码与排序。

import { type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { SortDirection } from "../sort";
import { parsePage, parseSortColumn, parseSortDirection, pushPage, pushSort } from "./primitives";

const LOTS_KEYS: readonly string[] = ["page", "sort", "dir"];

/** lot 列表可排序列：序号、状态、起拍价、当前价。 */
const LOTS_SORT_COLUMNS: readonly string[] = ["code", "status", "start_price", "current_price"];

/** lot 列表的页码与排序参数；空排序列表示保持服务端原始顺序。 */
export interface LotsQuery {
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的 offset 上限拒绝。 */
  page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序。合法值见 `LOTS_SORT_COLUMNS`。 */
  sort: string;
  direction: SortDirection;
}

/** 默认查询与 Rust `Default` 相同：第一页、服务端顺序、升序。 */
export function lotsQueryDefault(): LotsQuery {
  return { page: 1n, sort: "", direction: "asc" };
}

/** lot 列表的地址栏查询状态；刷新与前进/后退都会恢复页码与排序。 */
export function useLotsQueryState(): QueryState<LotsQuery> {
  return useQueryState(
    LOTS_KEYS,
    (pairs) => ({
      page: parsePage(pairs, "page"),
      sort: parseSortColumn(pairs, LOTS_SORT_COLUMNS),
      direction: parseSortDirection(pairs),
    }),
    (query) => {
      const pairs: [string, string][] = [];
      pushPage(pairs, "page", query.page);
      pushSort(pairs, query.sort, query.direction);
      return pairs;
    },
    { sameAddress: "write" },
  );
}
