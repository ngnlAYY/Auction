// realtime query：非法或重复筛选保留原值，但不得派生出列表请求。

import { expect, test } from "bun:test";
import { mountQueryState } from "./query-fixture";
import {
  realtimeQueryEventPageRequest,
  realtimeQueryFailurePageRequest,
  useRealtimeQueryState,
} from "./realtime";

function encode(pairs: readonly (readonly [string, string])[]): string {
  return new URLSearchParams(pairs.map(([key, value]) => [key, value])).toString();
}

test("恢复的筛选拒绝控制字符、失败码空格与未知状态", () => {
  const invalid: readonly (readonly [string, string])[] = [
    ["event_stream", "live\u007fstream"],
    ["event_kind", "live\tupdated"],
    ["event_kind", "\nupdated"],
    ["failure_stream", "live\u007fstream"],
    ["failure_consumer", "consumer\0name"],
    ["failure_consumer", "consumer\r"],
    ["failure_code", "some failure"],
    ["failure_state", "pending"],
  ];
  for (const pair of invalid) {
    const [key, value] = pair;
    const mounted = mountQueryState(
      `/admin/realtime?${encode([pair])}`,
      () => useRealtimeQueryState().location,
    );
    try {
      const query = mounted.state.state.peek();
      expect(realtimeQueryEventPageRequest(query) === null).toBe(key.startsWith("event_"));
      expect(realtimeQueryFailurePageRequest(query) === null).toBe(key.startsWith("failure_"));
      mounted.state.set(query, "push");
      expect(mounted.fake.writes.map(({ mode, url }) => [mode, [...url.searchParams]])).toEqual([
        ["push", [[key, value]]],
      ]);
    } finally {
      mounted.dispose();
    }
  }

  const mounted = mountQueryState(
    `/admin/realtime?${encode([
      ["event_kind", " 状态 更新 "],
      ["failure_code", " transient_error "],
      ["failure_state", "completed"],
    ])}`,
    () => useRealtimeQueryState().location,
  );
  try {
    const query = mounted.state.state.peek();
    expect(realtimeQueryEventPageRequest(query)?.[2]).toBe("状态 更新");
    const failure = realtimeQueryFailurePageRequest(query);
    expect(failure?.[3]).toBe("transient_error");
    expect(failure?.[4]).toBe("completed");
  } finally {
    mounted.dispose();
  }
});

test("恢复的重复键即使每个值合法也阻止对应列表", () => {
  const duplicated: readonly (readonly [string, string])[] = [
    ["event_stream", "live-stream"],
    ["event_kind", "updated"],
    ["failure_stream", "live-stream"],
    ["failure_consumer", "consumer"],
    ["failure_code", "transient_error"],
    ["failure_state", "completed"],
  ];
  for (const [key, value] of duplicated) {
    const repeated: [string, string][] = [
      [key, value],
      [key, value],
    ];
    const mounted = mountQueryState(
      `/admin/realtime?${encode(repeated)}`,
      () => useRealtimeQueryState().location,
    );
    try {
      const query = mounted.state.state.peek();
      expect(realtimeQueryEventPageRequest(query) === null).toBe(key.startsWith("event_"));
      expect(realtimeQueryFailurePageRequest(query) === null).toBe(key.startsWith("failure_"));
      mounted.state.set(query, "push");
      expect(mounted.fake.writes.map(({ mode, url }) => [mode, [...url.searchParams]])).toEqual([
        ["push", repeated],
      ]);
    } finally {
      mounted.dispose();
    }
  }
});
