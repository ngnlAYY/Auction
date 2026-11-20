// 玩家支持视图的 query 状态：检索入口只有用户与参考编号，二者互斥。

import { type QueryPair, type QueryState, useQueryState } from "@auction/web-shared/net/query";
import { validUuid } from "@auction/web-shared/ui/validation";

import { parseText, parseUuidText, pushText } from "./primitives";

const SUPPORT_KEYS: readonly string[] = ["user", "reference"];

/** 玩家支持检索入口；两个字段至多一个非空。 */
export interface SupportQuery {
  user: string;
  reference: string;
}

/** 默认空入口；与 Rust 的 `Default` 相同。 */
export function supportQueryDefault(): SupportQuery {
  return { user: "", reference: "" };
}

/** 语义校验：恰好一个非空入口；参考编号必须是规范 UUID。 */
export function supportQueryValid(query: SupportQuery): boolean {
  if (query.user !== "" && query.reference === "") {
    return true;
  }
  if (query.user === "" && query.reference !== "") {
    return validUuid(query.reference);
  }
  return false;
}

/** 地址栏只恢复格式合法的入口；是否恰好一个入口由 `supportQueryValid` 在请求前判定。 */
function parseSupportQuery(pairs: readonly QueryPair[]): SupportQuery {
  return {
    user: parseText(pairs, "user", 256),
    reference: parseUuidText(pairs, "reference"),
  };
}

function supportQueryPairs(query: SupportQuery): readonly QueryPair[] {
  const pairs: QueryPair[] = [];
  pushText(pairs, "user", query.user);
  pushText(pairs, "reference", query.reference);
  return pairs;
}

/** 玩家支持的 URL query 状态。 */
export function useSupportQueryState(): QueryState<SupportQuery> {
  return useQueryState(SUPPORT_KEYS, parseSupportQuery, supportQueryPairs, {
    sameAddress: "write",
  });
}
