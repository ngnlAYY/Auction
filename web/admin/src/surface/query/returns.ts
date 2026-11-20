// 结算退件列表的 query 状态：范围（缺省只看未结案）、状态、证据分类、义务种类、世界与页码。
// 取值不在契约闭集内时回退为“不限”，不把畸形文本转发给 API。

import {
  ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
  ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
  ADMIN_SETTLEMENT_RETURN_STATUSES,
} from "@auction/contracts/dto/admin/returns";
import { type QueryPair, type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { SettlementReturnFilters } from "../../api/returns";
import { pairValue, parseChoice, parsePage, pushPage, pushText } from "./primitives";

const RETURNS_KEYS: readonly string[] = [
  "scope",
  "status",
  "match_state",
  "obligation_kind",
  "world",
  "page",
];

/** 世界筛选闭集。 */
const WORLDS: readonly string[] = ["main", "isekai"];

/** 列表视图状态；空串表示该筛选不限。 */
export interface ReturnsQuery {
  /** 同时列出已结案案件；缺省只看未结案。 */
  all: boolean;
  status: string;
  matchState: string;
  obligationKind: string;
  world: string;
  /** 地址栏完整保留 `i64`；请求前由端点偏移上限决定是否可发。 */
  page: bigint;
}

/** 缺省只看未结案第一页。 */
export function returnsQueryDefault(): ReturnsQuery {
  return {
    all: false,
    status: "",
    matchState: "",
    obligationKind: "",
    world: "",
    page: 1n,
  };
}

/** 转为 API 筛选；页码另行传入。 */
export function returnsQueryFilters(query: ReturnsQuery): SettlementReturnFilters {
  return {
    openOnly: !query.all,
    status: query.status,
    matchState: query.matchState,
    obligationKind: query.obligationKind,
    world: query.world,
  };
}

function parseReturnsQuery(pairs: readonly QueryPair[]): ReturnsQuery {
  return {
    all: pairValue(pairs, "scope") === "all",
    status: parseChoice(pairs, "status", ADMIN_SETTLEMENT_RETURN_STATUSES),
    matchState: parseChoice(pairs, "match_state", ADMIN_SETTLEMENT_RETURN_MATCH_STATES),
    obligationKind: parseChoice(pairs, "obligation_kind", ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS),
    world: parseChoice(pairs, "world", WORLDS),
    page: parsePage(pairs, "page"),
  };
}

function returnsQueryPairs(query: ReturnsQuery): readonly QueryPair[] {
  const pairs: QueryPair[] = [];
  if (query.all) {
    pairs.push(["scope", "all"]);
  }
  pushText(pairs, "status", query.status);
  pushText(pairs, "match_state", query.matchState);
  pushText(pairs, "obligation_kind", query.obligationKind);
  pushText(pairs, "world", query.world);
  pushPage(pairs, "page", query.page);
  return pairs;
}

/** 结算退件列表的 query 状态句柄。 */
export function useReturnsQueryState(): QueryState<ReturnsQuery> {
  return useQueryState(RETURNS_KEYS, parseReturnsQuery, returnsQueryPairs, {
    sameAddress: "write",
  });
}
