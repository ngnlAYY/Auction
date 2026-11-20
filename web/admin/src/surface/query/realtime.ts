// realtime 视图的 query 状态：事件/失败/延迟三个分页各自独立入 URL，另含自动轮询间隔。

import { type QueryState, useQueryState } from "@auction/web-shared/net/query";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { Accessor } from "solid-js";

import {
  type FilterField,
  type FilterValue,
  filterFieldNormalize,
  filterValueAppendPairs,
  filterValueDefault,
  filterValueFromText,
  filterValueRestore,
  filterValueText,
} from "../realtime/filter-field";
import { parsePage, parsePollSeconds, pushPage, pushPollSeconds } from "./primitives";

const REALTIME_KEYS: readonly string[] = [
  "event_stream",
  "event_kind",
  "event_page",
  "failure_stream",
  "failure_consumer",
  "failure_code",
  "failure_state",
  "failure_page",
  "lag_page",
  "poll",
];

type EventPageRequest = readonly [bigint, string, string];
type FailurePageRequest = readonly [bigint, string, string, string, string];

/** realtime 地址栏状态：事件、失败、积压三个分页与自动轮询间隔各自独立。 */
export interface RealtimeQuery {
  eventStream: FilterValue;
  eventKind: FilterValue;
  eventPage: bigint;
  failureStream: FilterValue;
  failureConsumer: FilterValue;
  failureCode: FilterValue;
  /** 空串表示全部状态，非法历史值保留以便更正。 */
  failureState: FilterValue;
  failurePage: bigint;
  lagPage: bigint;
  /** 自动轮询间隔秒数；0 表示关闭（默认）。合法值见 `POLL_INTERVAL_VALUES`。 */
  pollSeconds: number;
}

/** realtime 三个分页各自独立入 URL，默认值都是第一页。 */
export function realtimeQueryDefault(): RealtimeQuery {
  return {
    eventStream: filterValueDefault(),
    eventKind: filterValueDefault(),
    eventPage: 1n,
    failureStream: filterValueDefault(),
    failureConsumer: filterValueDefault(),
    failureCode: filterValueDefault(),
    failureState: filterValueDefault(),
    failurePage: 1n,
    lagPage: 1n,
    pollSeconds: 0,
  };
}

/** 非法历史筛选保留原值，但不能派生出可发送的请求。 */
export function realtimeQueryEventPageRequest(query: RealtimeQuery): EventPageRequest | null {
  const stream = filterValueText(query.eventStream);
  const kind = filterValueText(query.eventKind);
  if (stream === null || kind === null) {
    return null;
  }
  const normalizedStream = filterFieldNormalize("event_stream", stream);
  const normalizedKind = filterFieldNormalize("event_kind", kind);
  if (normalizedStream === null || normalizedKind === null) {
    return null;
  }
  return [query.eventPage, normalizedStream, normalizedKind];
}

/** 失败列表需要全部已应用条件合法，避免以更宽条件读取记录。 */
export function realtimeQueryFailurePageRequest(query: RealtimeQuery): FailurePageRequest | null {
  const stream = filterValueText(query.failureStream);
  const consumer = filterValueText(query.failureConsumer);
  const code = filterValueText(query.failureCode);
  const state = filterValueText(query.failureState);
  if (stream === null || consumer === null || code === null || state === null) {
    return null;
  }
  const normalizedStream = filterFieldNormalize("failure_stream", stream);
  const normalizedConsumer = filterFieldNormalize("failure_consumer", consumer);
  const normalizedCode = filterFieldNormalize("failure_code", code);
  const normalizedState = filterFieldNormalize("failure_state", state);
  if (
    normalizedStream === null ||
    normalizedConsumer === null ||
    normalizedCode === null ||
    normalizedState === null
  ) {
    return null;
  }
  return [query.failurePage, normalizedStream, normalizedConsumer, normalizedCode, normalizedState];
}

/** 应用事件筛选并返回事件列表的第一页。 */
export function realtimeQueryWithEventFilters(
  query: RealtimeQuery,
  streamKind: readonly [string, string],
): RealtimeQuery {
  const [stream, kind] = streamKind;
  return {
    ...query,
    eventStream: filterValueFromText(stream),
    eventKind: filterValueFromText(kind),
    eventPage: 1n,
  };
}

/** 应用失败文本筛选并返回失败列表的第一页。 */
export function realtimeQueryWithFailureFilters(
  query: RealtimeQuery,
  streamConsumerCode: readonly [string, string, string],
): RealtimeQuery {
  const [stream, consumer, code] = streamConsumerCode;
  return {
    ...query,
    failureStream: filterValueFromText(stream),
    failureConsumer: filterValueFromText(consumer),
    failureCode: filterValueFromText(code),
    failurePage: 1n,
  };
}

/** 地址栏状态与两个表单实际订阅的筛选。 */
export interface RealtimeQueryState {
  location: QueryState<RealtimeQuery>;
  eventFilters: Accessor<readonly [FilterValue, FilterValue]>;
  failureFilters: Accessor<readonly [FilterValue, FilterValue, FilterValue, FilterValue]>;
}

/** 创建 realtime 查询状态；表单只跟随各自已应用的筛选，翻页与轮询不重写表单。 */
export function useRealtimeQueryState(): RealtimeQueryState {
  const location = useQueryState(REALTIME_KEYS, parseRealtimeQuery, realtimeQueryPairs, {
    sameAddress: "write",
  });
  const state = location.state;
  // 翻页和轮询设置会更新地址栏，但表单只跟随各自已应用的筛选。
  const eventFilters = createValueMemo((): readonly [FilterValue, FilterValue] => {
    const query = state();
    return [query.eventStream, query.eventKind];
  });
  const failureFilters = createValueMemo(
    (): readonly [FilterValue, FilterValue, FilterValue, FilterValue] => {
      const query = state();
      return [query.failureStream, query.failureConsumer, query.failureCode, query.failureState];
    },
  );
  return { location, eventFilters, failureFilters };
}

function realtimeQueryPairs(query: RealtimeQuery): [string, string][] {
  const pairs: [string, string][] = [];
  filterValueAppendPairs(query.eventStream, pairs, "event_stream");
  filterValueAppendPairs(query.eventKind, pairs, "event_kind");
  pushPage(pairs, "event_page", query.eventPage);
  filterValueAppendPairs(query.failureStream, pairs, "failure_stream");
  filterValueAppendPairs(query.failureConsumer, pairs, "failure_consumer");
  filterValueAppendPairs(query.failureCode, pairs, "failure_code");
  filterValueAppendPairs(query.failureState, pairs, "failure_state");
  pushPage(pairs, "failure_page", query.failurePage);
  pushPage(pairs, "lag_page", query.lagPage);
  pushPollSeconds(pairs, query.pollSeconds);
  return pairs;
}

function parseRealtimeQuery(pairs: readonly (readonly [string, string])[]): RealtimeQuery {
  const filter = (key: string, field: FilterField): FilterValue =>
    filterValueRestore(pairs, key, (value: string) => filterFieldNormalize(field, value));
  return {
    eventStream: filter("event_stream", "event_stream"),
    eventKind: filter("event_kind", "event_kind"),
    eventPage: parsePage(pairs, "event_page"),
    failureStream: filter("failure_stream", "failure_stream"),
    failureConsumer: filter("failure_consumer", "failure_consumer"),
    failureCode: filter("failure_code", "failure_code"),
    failureState: filter("failure_state", "failure_state"),
    failurePage: parsePage(pairs, "failure_page"),
    lagPage: parsePage(pairs, "lag_page"),
    pollSeconds: parsePollSeconds(pairs),
  };
}
