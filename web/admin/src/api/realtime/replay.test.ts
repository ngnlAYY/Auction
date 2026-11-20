// 重放命令的线上回执、原事件核对和发送前拒绝。

import { expect, test } from "bun:test";
import { createAdminRequestHarness, type Reply, TEST_SESSION_BODY } from "../api-request-fixture";
import { classifyAdminWriteFailure, writeOutcomeUnknown } from "../failure";
import { prepareRealtimeReplay, replayRealtimeFailure } from "./replay";

const COMMAND_ID = "00000000-0000-4000-8000-000000000029";
const TARGET = {
  id: "00000000-0000-4000-8000-000000000027",
  event_id: "00000000-0000-4000-8000-000000000026",
  stream_key: "auction:0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b",
};
const REPLAY = {
  event_id: "00000000-0000-4000-8000-000000000028",
  replay_of_event_id: TARGET.event_id,
  stream_key: TARGET.stream_key,
  stream_sequence: 14,
  kind: "lot.bid_accepted",
  schema_version: 3,
  created_at: "2025-11-15T09:46:40.110000Z",
};

function receipt(fields: object): string {
  return JSON.stringify({ code: "ok", message: "已重放", data: { replay: fields } });
}

async function exchange(reply: Reply | null, draft = '{"checked":true}') {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const requests = harness.installFetch(
      reply === null ? [] : [{ body: JSON.stringify(TEST_SESSION_BODY) }, reply],
    );
    const { host } = harness.registerHost();
    const result = await replayRealtimeFailure(
      host,
      TARGET,
      COMMAND_ID,
      prepareRealtimeReplay(3, draft),
      new AbortController().signal,
    );
    expect(requests).toEqual(
      reply === null
        ? []
        : [
            { method: "GET", url: "/api/v1/admin/session?include=server_time", body: "" },
            {
              method: "POST",
              url: `/api/v1/admin/realtime/failures/${TARGET.id}/replay`,
              body: '{"command_id":"00000000-0000-4000-8000-000000000029","schema_version":3,"public_payload":{"checked":true}}',
            },
          ],
    );
    return result;
  } finally {
    harness.cleanup();
  }
}

test("realtime_replay_accepts_only_matching_original_event_stream_and_submitted_schema", async () => {
  expect(await exchange({ body: receipt(REPLAY) })).toEqual({ ok: true, value: undefined });
  for (const [label, fields] of [
    ["缺失原事件", { ...REPLAY, replay_of_event_id: undefined }],
    ["原事件为 null", { ...REPLAY, replay_of_event_id: null }],
    ["原事件为空", { ...REPLAY, replay_of_event_id: "" }],
    ["原事件非法", { ...REPLAY, replay_of_event_id: "not-a-uuid" }],
    ["原事件错配", { ...REPLAY, replay_of_event_id: COMMAND_ID }],
    ["追加事件等于原事件", { ...REPLAY, event_id: TARGET.event_id }],
    ["流错配", { ...REPLAY, stream_key: "auction:other" }],
    ["回执使用旧 schema", { ...REPLAY, schema_version: 2 }],
  ] as const) {
    expect(await exchange({ body: receipt(fields) }), label).toEqual({
      ok: false,
      error: writeOutcomeUnknown("invalid_response"),
    });
  }
});

test("realtime_replay_rejects_fractional_event_sequence", async () => {
  expect((await exchange({ body: receipt(REPLAY) })).ok).toBe(true);
  const body = receipt(REPLAY).replace('"stream_sequence":14', '"stream_sequence":14.0');
  expect(await exchange({ body })).toEqual({
    ok: false,
    error: writeOutcomeUnknown("invalid_response"),
  });
});

test("realtime_replay_distinguishes_unsent_json_from_verified_conflict", async () => {
  const unsent = await exchange(null, "{");
  expect(unsent).toEqual({ ok: false, error: { kind: "not-sent", code: "invalid_request" } });
  const conflict = await exchange({
    status: 409,
    body: '{"code":"conflict","message":"命令冲突","data":null,"request_id":null}',
  });
  expect(conflict.ok).toBe(false);
  if (conflict.ok) throw new Error("冲突回执不应成功");
  expect(classifyAdminWriteFailure(conflict.error)).toBe("replay_checked");
});
