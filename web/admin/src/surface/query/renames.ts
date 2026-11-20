// 改名历史的 query 状态：用户名筛选、页码与排序。名称是即时筛选，每次输入以 replace
// 写入 URL 并回到第 1 页。

import { type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { SortDirection } from "../sort";
import {
  parsePage,
  parseSortColumn,
  parseSortDirection,
  parseText,
  pushPage,
  pushSort,
  pushText,
} from "./primitives";

const RENAMES_KEYS: readonly string[] = ["name", "page", "sort", "dir"];

/** 改名历史可排序列：改名时间、原用户名、新用户名、操作者。 */
const RENAMES_SORT_COLUMNS: readonly string[] = ["created", "previous", "next", "actor"];

/** 改名历史的筛选、页码与排序。 */
export interface RenamesQuery {
  /** 即时用户名筛选；空串表示不过滤。 */
  name: string;
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的 offset 上限拒绝。 */
  page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序。合法值见 `RENAMES_SORT_COLUMNS`。 */
  sort: string;
  direction: SortDirection;
}

/** 默认查询与 Rust `Default` 相同：无名称筛选、第一页、服务端顺序、升序。 */
export function renamesQueryDefault(): RenamesQuery {
  return { name: "", page: 1n, sort: "", direction: "asc" };
}

/** 改名历史的 URL query 状态：名称筛选在每次输入时 replace，并回到第 1 页。 */
export function useRenamesQueryState(): QueryState<RenamesQuery> {
  return useQueryState(
    RENAMES_KEYS,
    (pairs) => ({
      name: parseText(pairs, "name", 256),
      page: parsePage(pairs, "page"),
      sort: parseSortColumn(pairs, RENAMES_SORT_COLUMNS),
      direction: parseSortDirection(pairs),
    }),
    (query) => {
      const pairs: [string, string][] = [];
      pushText(pairs, "name", query.name);
      pushPage(pairs, "page", query.page);
      pushSort(pairs, query.sort, query.direction);
      return pairs;
    },
    { sameAddress: "write" },
  );
}
