// 玩家支持视图的 query 状态：检索入口只有用户与参考编号，二者互斥。

import { type QueryPair, type QueryState, useQueryState } from "@auction/web-shared/net/query";
import { validUuid } from "@auction/web-shared/ui/validation";

import { playerSupportUserTooLong } from "../../api/delivery";
import { trimRustWhitespace } from "../../api/session-time";
import { pairValue, parseUuidText, pushText } from "./primitives";

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

/** 语义校验：恰好一个非空入口，用户检索词须符合字节上限，参考编号须为规范 UUID。 */
export function supportQueryValid(query: SupportQuery): boolean {
  if (query.user !== "" && query.reference === "") {
    return !playerSupportUserTooLong(query.user);
  }
  if (query.user === "" && query.reference !== "") {
    return validUuid(query.reference);
  }
  return false;
}

/** 保留超长检索词以显示字段错误；请求前由 `supportQueryValid` 拒绝该入口。 */
function parseSupportQuery(pairs: readonly QueryPair[]): SupportQuery {
  const user = trimRustWhitespace(pairValue(pairs, "user") ?? "");
  return {
    user: !playerSupportUserTooLong(user) && /\p{Cc}/u.test(user) ? "" : user,
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
