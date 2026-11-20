// 告警台的 query 状态：状态筛选（闭集，默认 `active`）与页码。
// 备注与静默时长是提交前草稿，不入 URL。

import type { QueryPair, QueryState } from "@auction/web-shared/net/query";
import { useQueryState } from "@auction/web-shared/net/query";

import type { AdminAlertStatus } from "../../api/alerts";
import { pairValue, parsePage, pushPage } from "./primitives";

const ALERTS_KEYS: readonly string[] = ["status", "page"];

/** 告警台状态筛选闭集；与服务端 `status` 参数一致。 */
export const ALERT_STATUS_VALUES = [
  "active",
  "resolved",
  "all",
] as const satisfies readonly AdminAlertStatus[];

/** 缺省筛选只看活动告警；等于缺省值时不写地址栏。 */
export const DEFAULT_ALERT_STATUS: AdminAlertStatus = "active";

/** 告警台的筛选与页码。 */
export interface AlertsQuery {
  /** 合法值见 {@link ALERT_STATUS_VALUES}。 */
  readonly status: AdminAlertStatus;
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的偏移上限拒绝。 */
  readonly page: bigint;
}

/** 缺省查询与 Rust `Default` 相同：活动告警、第一页。 */
export function alertsQueryDefault(): AlertsQuery {
  return { status: DEFAULT_ALERT_STATUS, page: 1n };
}

/** 把输入映射到状态闭集；未知值返回 `null`，由调用方决定回退或忽略。 */
export function alertStatusChoice(value: string): AdminAlertStatus | null {
  return ALERT_STATUS_VALUES.find((status) => status === value) ?? null;
}

/** 非法状态回退 `active`，非法页码回退第一页。 */
function parseAlertsQuery(pairs: readonly QueryPair[]): AlertsQuery {
  return {
    status: alertStatusChoice(pairValue(pairs, "status") ?? "") ?? DEFAULT_ALERT_STATUS,
    page: parsePage(pairs, "page"),
  };
}

function alertsQueryPairs(query: AlertsQuery): readonly QueryPair[] {
  const pairs: QueryPair[] = [];
  if (query.status !== DEFAULT_ALERT_STATUS) {
    pairs.push(["status", query.status]);
  }
  pushPage(pairs, "page", query.page);
  return pairs;
}

/** 告警台的筛选与页码；从地址栏读取并写回。 */
export function useAlertsQueryState(): QueryState<AlertsQuery> {
  return useQueryState(ALERTS_KEYS, parseAlertsQuery, alertsQueryPairs, {
    sameAddress: "write",
  });
}
