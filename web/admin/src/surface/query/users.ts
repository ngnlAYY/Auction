// 用户列表的 query 状态：用户名、状态筛选、页码与排序。名称输入框是提交前草稿，不入 URL。

import type { AdminUserPaginationResponse } from "@auction/contracts/dto/admin/users";
import {
  type QueryPair,
  type QueryState,
  useQueryState,
  writeQueryPairs,
} from "@auction/web-shared/net/query";
import { unreachable } from "@auction/web-shared/result";

import type { AdminUserStatusFilter } from "../../api/users";
import type { SortDirection } from "../sort";
import { USER_NAME_BYTES } from "../users/fields";
import {
  pairValue,
  parsePage,
  parseSortColumn,
  parseSortDirection,
  parseText,
  pushPage,
  pushSort,
  pushText,
} from "./primitives";

const USERS_KEYS: readonly string[] = ["name", "status", "page", "sort", "dir"];

/** 用户列表可排序列：用户名、状态、拉黑次数、创建时间。 */
const USERS_SORT_COLUMNS: readonly string[] = [
  "username",
  "status",
  "blacklist_count",
  "created_at",
];

/** 用户列表的筛选、页码与排序。 */
export interface UsersQuery {
  /** 用户名筛选；空串表示不过滤。 */
  name: string;
  /** 状态筛选；`all` 是默认值。 */
  status: AdminUserStatusFilter;
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的 offset 上限拒绝。 */
  page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序。合法值见 `USERS_SORT_COLUMNS`。 */
  sort: string;
  direction: SortDirection;
}

/** 默认查询与 Rust `Default` 相同：无名称筛选、全部状态、第一页、服务端顺序、升序。 */
export function usersQueryDefault(): UsersQuery {
  return { name: "", status: "all", page: 1n, sort: "", direction: "asc" };
}

/** 地址栏写入成功后发布新条件；浏览历史恢复与首次挂载仍共用同一解析器。
 * Rust 对相同查询也会成功 push，因此同址提交同样创建历史项。 */
export function usersQueryApply(query: UsersQuery, state: QueryState<UsersQuery>): void {
  if (!writeQueryPairs(USERS_KEYS, serializeUsersQuery(query), "push", { sameAddress: "write" })) {
    return;
  }
  state.state.set(query);
}

/** 只有与当前请求同页的权威响应才有资格纠正页码；返回值是目标页，`null` 表示无需纠正。 */
export function usersQueryPageCorrection(
  query: UsersQuery,
  response: AdminUserPaginationResponse,
): bigint | null {
  if (
    !Number.isSafeInteger(response.page) ||
    !Number.isSafeInteger(response.page_size) ||
    !Number.isSafeInteger(response.total) ||
    BigInt(response.page) !== query.page
  ) {
    return null;
  }
  const lastPage =
    1n + (BigInt(Math.max(response.total, 1)) - 1n) / BigInt(Math.max(response.page_size, 1));
  return query.page > lastPage ? lastPage : null;
}

/** 无需纠页或成功写回时返回 true；写入失败时保留原查询。 */
export function usersQueryCorrectPage(
  query: UsersQuery,
  state: QueryState<UsersQuery>,
  response: AdminUserPaginationResponse,
): boolean {
  const page = usersQueryPageCorrection(query, response);
  if (page === null) {
    return true;
  }
  // 后退落在超出结果范围的页时，原地修正以保留之后的历史项。
  const corrected = { ...query, page };
  if (
    !writeQueryPairs(USERS_KEYS, serializeUsersQuery(corrected), "replace", {
      sameAddress: "write",
    })
  ) {
    return false;
  }
  state.state.set(corrected);
  return true;
}

/** 状态筛选序列化：`all` 是默认值，不写地址栏。 */
function usersStatusValue(status: AdminUserStatusFilter): "active" | "blacklisted" | null {
  switch (status) {
    case "all":
      return null;
    case "active":
      return "active";
    case "blacklisted":
      return "blacklisted";
    default:
      return unreachable(status);
  }
}

function parseUsersQuery(pairs: readonly QueryPair[]): UsersQuery {
  return {
    name: parseText(pairs, "name", USER_NAME_BYTES),
    status: parseUsersStatus(pairValue(pairs, "status")),
    page: parsePage(pairs, "page"),
    sort: parseSortColumn(pairs, USERS_SORT_COLUMNS),
    direction: parseSortDirection(pairs),
  };
}

function parseUsersStatus(value: string | null): AdminUserStatusFilter {
  if (value === "active") {
    return "active";
  }
  if (value === "blacklisted") {
    return "blacklisted";
  }
  return "all";
}

function serializeUsersQuery(query: UsersQuery): readonly QueryPair[] {
  const pairs: QueryPair[] = [];
  pushText(pairs, "name", query.name);
  const status = usersStatusValue(query.status);
  if (status !== null) {
    pairs.push(["status", status]);
  }
  pushPage(pairs, "page", query.page);
  pushSort(pairs, query.sort, query.direction);
  return pairs;
}

/** 用户列表的 URL query 状态。 */
export function useUsersQueryState(): QueryState<UsersQuery> {
  return useQueryState(USERS_KEYS, parseUsersQuery, serializeUsersQuery, {
    sameAddress: "write",
  });
}
