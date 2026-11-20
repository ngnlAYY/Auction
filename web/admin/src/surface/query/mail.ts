// 邮件入站列表的 query 状态：世界、类型、状态筛选，页码与排序。
// 邮箱轮询输入、详情面板开合是瞬态，不入 URL。

import { type QueryState, useQueryState } from "@auction/web-shared/net/query";

import { ADMIN_MAIL_INTAKE_KIND_VALUES, type AdminMailIntakeQuery } from "../../api/mail";
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

const MAIL_KEYS: readonly string[] = ["world", "kind", "status", "page", "sort", "dir"];

const MAIL_STATUS_VALUES: readonly string[] = ["accepted", "rejected", "failed"];

/** 邮件入站列表可排序列：接收时间、类型、状态、世界。 */
const MAIL_SORT_COLUMNS: readonly string[] = ["received", "kind", "status", "world"];

/** 邮件入站列表的筛选、页码与排序。 */
export interface MailQuery {
  /** 空串表示全部世界；合法值为 `main` / `isekai`。 */
  world: string;
  /** 空串表示全部类型；合法值见 {@link ADMIN_MAIL_INTAKE_KIND_VALUES}。 */
  kind: string;
  /** 空串表示全部状态；合法值见 `MAIL_STATUS_VALUES`。 */
  status: string;
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的 offset 上限拒绝。 */
  page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序。合法值见 `MAIL_SORT_COLUMNS`。 */
  sort: string;
  /** 排序方向；仅 `sort` 非空时有意义。 */
  direction: SortDirection;
}

/** 默认查询与 Rust `Default` 相同：全部世界、全部类型、全部状态、第一页、服务端顺序、升序。 */
export function mailQueryDefault(): MailQuery {
  return {
    world: "",
    kind: "",
    status: "",
    page: 1n,
    sort: "",
    direction: "asc",
  };
}

/** 映射为入站列表的 API 查询；排序与页码不转发给筛选。 */
export function mailQueryToIntakeQuery(query: MailQuery): AdminMailIntakeQuery {
  return {
    worldId: query.world,
    kind: query.kind,
    status: query.status,
  };
}

/** 邮件入站列表的筛选、页码与排序；从地址栏读取并写回。 */
export function useMailQueryState(): QueryState<MailQuery> {
  return useQueryState(
    MAIL_KEYS,
    (pairs) => ({
      world: parseChoice(pairs, "world", ["main", "isekai"]),
      kind: parseChoice(pairs, "kind", ADMIN_MAIL_INTAKE_KIND_VALUES),
      status: parseChoice(pairs, "status", MAIL_STATUS_VALUES),
      page: parsePage(pairs, "page"),
      sort: parseSortColumn(pairs, MAIL_SORT_COLUMNS),
      direction: parseSortDirection(pairs),
    }),
    (query) => {
      const pairs: [string, string][] = [];
      pushText(pairs, "world", query.world);
      pushText(pairs, "kind", query.kind);
      pushText(pairs, "status", query.status);
      pushPage(pairs, "page", query.page);
      pushSort(pairs, query.sort, query.direction);
      return pairs;
    },
    { sameAddress: "write" },
  );
}
