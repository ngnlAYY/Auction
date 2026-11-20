// 仪表盘的 query 状态：快照本身是资源瞬态，只有轮询开关入 URL。

import type { QueryPair, QueryState } from "@auction/web-shared/net/query";
import { useQueryState } from "@auction/web-shared/net/query";

import { parsePollSecondsWithDefault } from "./primitives";

const DASHBOARD_KEYS: readonly string[] = ["poll"];

/** 仪表盘的自动轮询间隔；合法值见 `POLL_INTERVAL_VALUES`，0 表示关闭。 */
export interface DashboardQuery {
  readonly pollSeconds: number;
}

/** 与 Rust `DashboardQuery::default` 相同：缺省秒数为 0；URL 缺省由解析器回退到 30 秒。 */
export function dashboardQueryDefault(): DashboardQuery {
  return { pollSeconds: 0 };
}

/** 解析地址栏：显式 `poll=0` 才关闭默认轮询，缺省或非法值回退到 30 秒。 */
function parseDashboardQuery(pairs: readonly QueryPair[]): DashboardQuery {
  return { pollSeconds: parsePollSecondsWithDefault(pairs, 30) };
}

/** 写回地址栏时也保存 0，避免刷新把关闭状态解析回默认开启。 */
function dashboardQueryPairs(query: DashboardQuery): readonly QueryPair[] {
  return [["poll", query.pollSeconds.toString()]];
}

/** 仪表盘的轮询开关；从地址栏读取并在变更时写回。 */
export function useDashboardQueryState(): QueryState<DashboardQuery> {
  return useQueryState(DASHBOARD_KEYS, parseDashboardQuery, dashboardQueryPairs, {
    sameAddress: "write",
  });
}
