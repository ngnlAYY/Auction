// 只含页码与排序的列表页 query 状态（凭据列表）。

import { type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { SortDirection } from "../sort";
import { parsePage, parseSortColumn, parseSortDirection, pushPage, pushSort } from "./primitives";

const PAGE_ONLY_KEYS: readonly string[] = ["page", "sort", "dir"];

/** 凭据列表可排序列：标签、世界、类型、状态、上次检查时间。 */
const CREDENTIALS_SORT_COLUMNS: readonly string[] = ["label", "world", "kind", "status", "checked"];

/** 凭据列表的页码与排序。 */
export interface PageQuery {
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的 offset 上限拒绝。 */
  page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序。合法值见 `CREDENTIALS_SORT_COLUMNS`。 */
  sort: string;
  direction: SortDirection;
}

/** 默认查询与 Rust `Default` 相同：第一页、服务端顺序、升序。 */
export function pageQueryDefault(): PageQuery {
  return { page: 1n, sort: "", direction: "asc" };
}

/** 凭据列表的 URL query 状态：只读取 page、sort、dir，并按相同顺序写回。 */
export function usePageQueryState(): QueryState<PageQuery> {
  return useQueryState(
    PAGE_ONLY_KEYS,
    (pairs) => ({
      page: parsePage(pairs, "page"),
      sort: parseSortColumn(pairs, CREDENTIALS_SORT_COLUMNS),
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
