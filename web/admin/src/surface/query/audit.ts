// 审计列表的 query 状态：九项筛选、页码与排序。
// 表单草稿只在提交时写入 URL，无效历史值保留供操作员更正。

import { type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { AdminAuditFilter } from "../../api/audit";
import {
  type AuditFilterState,
  auditFilterStateAppendPairs,
  auditFilterStateDefault,
  auditFilterStateFromFilter,
  auditFilterStateRequest,
  auditFilterStateRestore,
} from "../audit/filter-state";
import type { SortDirection } from "../sort";
import { parsePage, parseSortColumn, parseSortDirection, pushPage, pushSort } from "./primitives";

const AUDIT_KEYS: readonly string[] = [
  "actor",
  "entity",
  "action",
  "request_id",
  "actor_type",
  "source",
  "result",
  "from",
  "to",
  "page",
  "sort",
  "dir",
];

/** 审计列表可排序列：发生时间、操作者、动作、实体、来源、结果。 */
const AUDIT_SORT_COLUMNS: readonly string[] = [
  "at",
  "actor",
  "action",
  "entity",
  "source",
  "result",
];

/** 审计列表的筛选、页码与排序。 */
export interface AuditQuery {
  readonly filter: AuditFilterState;
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码由 API 的 offset 上限拒绝。 */
  readonly page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序（发生时间倒序）。 */
  readonly sort: string;
  readonly direction: SortDirection;
}

/** 默认查询与 Rust `Default` 相同：无筛选、第一页、服务端顺序、升序。 */
export function auditQueryDefault(): AuditQuery {
  return { filter: auditFilterStateDefault(), page: 1n, sort: "", direction: "asc" };
}

/** 提交筛选从第一页开始，并沿用当前排序。 */
export function auditQueryWithFilter(query: AuditQuery, filter: AdminAuditFilter): AuditQuery {
  return { ...query, filter: auditFilterStateFromFilter(filter), page: 1n };
}

/** 无效地址等待更正，不能按缺省筛选读取审计记录。 */
export function auditQueryPageRequest(
  query: AuditQuery,
): readonly [page: bigint, filter: AdminAuditFilter] | null {
  const filter = auditFilterStateRequest(query.filter);
  return filter === null ? null : ([query.page, filter] as const);
}

/** 审计列表的 URL query 状态：筛选、页码与排序共同恢复。 */
export function useAuditQueryState(): QueryState<AuditQuery> {
  return useQueryState(
    AUDIT_KEYS,
    (pairs) => ({
      filter: auditFilterStateRestore(pairs),
      page: parsePage(pairs, "page"),
      sort: parseSortColumn(pairs, AUDIT_SORT_COLUMNS),
      direction: parseSortDirection(pairs),
    }),
    (query) => {
      const pairs: [string, string][] = [];
      auditFilterStateAppendPairs(query.filter, pairs);
      pushPage(pairs, "page", query.page);
      pushSort(pairs, query.sort, query.direction);
      return pairs;
    },
    { sameAddress: "write" },
  );
}
