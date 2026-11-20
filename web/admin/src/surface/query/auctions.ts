// 拍卖列表（场次）的 query 状态：世界筛选、页码与排序。

import { type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { SortDirection } from "../sort";
import {
  parseChoice,
  parsePage,
  parseSortColumn,
  parseSortDirection,
  pushPage,
  pushSort,
  pushText,
} from "./primitives";

const AUCTIONS_KEYS: readonly string[] = ["world", "page", "sort", "dir"];

/** 拍卖列表可排序列：场次号、状态、开拍时间、结拍时间。 */
const AUCTIONS_SORT_COLUMNS: readonly string[] = ["sequence", "status", "open_at", "end_at"];

/** 场次列表的筛选、页码与排序。 */
export interface AuctionsQuery {
  /** 空串表示全部世界；合法值为 `main` / `isekai`。 */
  world: string;
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的 offset 上限拒绝。 */
  page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序。合法值见 `AUCTIONS_SORT_COLUMNS`。 */
  sort: string;
  /** 排序方向；仅 `sort` 非空时有意义。 */
  direction: SortDirection;
}

/** 默认查询与 Rust `Default` 相同：全部世界、第一页、服务端顺序、升序。 */
export function auctionsQueryDefault(): AuctionsQuery {
  return { world: "", page: 1n, sort: "", direction: "asc" };
}

export function useAuctionsQueryState(): QueryState<AuctionsQuery> {
  return useQueryState(
    AUCTIONS_KEYS,
    (pairs) => ({
      world: parseChoice(pairs, "world", ["main", "isekai"]),
      page: parsePage(pairs, "page"),
      sort: parseSortColumn(pairs, AUCTIONS_SORT_COLUMNS),
      direction: parseSortDirection(pairs),
    }),
    (query) => {
      const pairs: [string, string][] = [];
      pushText(pairs, "world", query.world);
      pushPage(pairs, "page", query.page);
      pushSort(pairs, query.sort, query.direction);
      return pairs;
    },
    { sameAddress: "write" },
  );
}
