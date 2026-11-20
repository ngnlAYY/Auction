// 实时读取回执使用与 Rust Admin 相同的契约样本和拒绝层次。

import { expect, test } from "bun:test";
import { AdminRealtimeFailureListResponseSchema } from "@auction/contracts/dto/admin/realtime";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { parseJson } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";

import { fixtureText, success } from "../auction-contract-fixture.test";
import { jsonArray, jsonObject } from "../json-fixture";
import {
  decodeRealtimeEventPage,
  decodeRealtimeFailurePage,
  decodeRealtimeLagPage,
} from "./decode";

function response(text: string): TransportResponse {
  return {
    status: 200,
    statusText: "OK",
    headers: new Headers(),
    body: new TextEncoder().encode(text),
    text: () => text,
    json(policy) {
      const parsed = parseJson(text, policy);
      return parsed.ok ? parsed : err({ kind: "decode" });
    },
  };
}

test("realtime_reads_decode_backend_contract_fixtures", () => {
  const events = success(
    decodeRealtimeEventPage(
      response(fixtureText("realtime-events")),
      1,
      50,
      "",
      "lot.bid_accepted",
    ),
  );
  expect(events.events[0]?.schema_version).toBe(2);

  const lag = success(
    decodeRealtimeLagPage(
      response(fixtureText("realtime-lag")),
      1,
      50,
      "auction:",
      "public-realtime",
    ),
  );
  expect(lag.lags[0]?.lag).toBe(2);

  for (const [sample, resolved, pageSize, hasMore] of [
    ["realtime-failures", false, 1, true],
    ["realtime-failures-resolved", true, 50, false],
  ] as const) {
    const page = success(
      decodeRealtimeFailurePage(
        response(fixtureText(sample)),
        1,
        pageSize,
        "",
        "public-realtime",
        "",
        resolved,
      ),
    );
    const first = page.failures[0];
    if (first === undefined) {
      throw new Error("契约样本缺少首条失败记录");
    }
    if (resolved) {
      expect(typeof first.resolved_at).toBe("string");
    } else {
      expect(first.resolved_at).toBeNull();
    }
    expect(page.pagination.has_more).toBe(hasMore);
  }
});

test("realtime_failure_rejects_invalid_resolution_fields", () => {
  const payload = jsonObject(success(parseJson(fixtureText("realtime-failures"))), "回执");
  const data = jsonObject(payload.data, "失败列表");
  const first = jsonObject(jsonArray(data.failures, "失败记录")[0], "首条失败记录");
  const schema = apiResponseSchema(AdminRealtimeFailureListResponseSchema);
  first.resolved = true;
  expect(decode(schema, payload).ok).toBe(true);
  expect(
    decodeRealtimeFailurePage(response(JSON.stringify(payload)), 1, 1, "", "", "", null),
  ).toEqual({ ok: false, error: "realtime_failure" });

  first.resolved_at = 17;
  expect(decode(schema, payload)).toEqual({ ok: false });
});
