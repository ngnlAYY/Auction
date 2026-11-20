// 模板写入：真实发送同一份冻结正文；畸形回执保持未决，有效拒绝才解除未知。

import { expect, jest, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";
import { flush } from "solid-js";
import { installBrowserRequest, installWriteFetch } from "../../api/api-request-fixture";
import {
  prepareMailTemplateAttemptOnce,
  submitMailTemplate,
} from "../../api/notification-templates";
import { cleanupScope } from "../../test-scope-fixture";
import { createPendingHarness, settlePending } from "./pending-command-fixture";

const ENTITY = "mail-template:auction_no_attachments";
const URL = "https://auction.test/api/v1/admin/mail-templates/auction_no_attachments";
const MALFORMED = Uint8Array.from([
  ...new TextEncoder().encode('{"code":"mail_template_conflict","message":"'),
  0xff,
  ...new TextEncoder().encode('","data":null,"request_id":null}'),
]);
const REPLACEMENT = new TextEncoder().encode(
  '{"code":"mail_template_conflict","message":"\ufffd","data":null,"request_id":null}',
);

async function settleMicrotasks(): Promise<void> {
  for (let round = 0; round < 20; round += 1) {
    await Promise.resolve();
    flush();
  }
}

async function settleRealTimers(): Promise<void> {
  for (let round = 0; round < 12; round += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    flush();
  }
  await settlePending();
}

test("畸形 UTF-8 保留模板命令直到收到有效拒绝", async () => {
  const scope = cleanupScope();
  try {
    scope.defer(flush);
    jest.useFakeTimers();
    scope.defer(() => jest.useRealTimers());
    scope.defer(installBrowserRequest());
    const harness = createPendingHarness();
    scope.defer(harness.dispose);
    const fetch = installWriteFetch((_request, init, index) => {
      if (index === 0) {
        // 首次写入不返回头；传输层超时中止后按 request_timeout 结束本次尝试。
        return new Promise<Response>((_resolve, reject) => {
          const signal = init?.signal;
          if (signal?.aborted === true) {
            reject(new DOMException("aborted", "AbortError"));
            return;
          }
          signal?.addEventListener(
            "abort",
            () => reject(new DOMException("aborted", "AbortError")),
            { once: true },
          );
        });
      }
      return new Response(index === 1 ? MALFORMED : REPLACEMENT, {
        status: 409,
        headers: { "content-type": "application/json" },
      });
    });
    scope.defer(fetch.restore);
    let successCalls = 0;
    const confirmed = Object.freeze({
      key: "auction_no_attachments",
      expectedRevision: 5,
      body: "首次确认的正文",
    });
    let prepare: ReturnType<typeof prepareMailTemplateAttemptOnce> | null = null;
    expect(
      harness.origin.commands.submit(
        ENTITY,
        "保存模板",
        async (commandId, signal) => {
          if (prepare === null) {
            prepare = prepareMailTemplateAttemptOnce({
              commandId,
              ...confirmed,
            });
          }
          const prepared = prepare(commandId);
          if (!prepared.ok) {
            return prepared;
          }
          return submitMailTemplate(harness.host.sessionHost, prepared.value, signal);
        },
        () => {
          successCalls += 1;
          throw new Error("冲突不能交付成功回执");
        },
        () => {},
      ),
    ).toBe(true);

    await settleMicrotasks();
    expect(fetch.writes).toHaveLength(1);
    jest.advanceTimersByTime(10_000);
    await settleMicrotasks();
    const commandId = harness.host.entries.peek()[0]?.identity.commandId;
    if (commandId === undefined) throw new Error("未生成模板命令 ID");
    expect(harness.host.entries.peek()[0]?.unknown).toBe(true);
    jest.useRealTimers();

    harness.host.retry(commandId);
    await settleRealTimers();
    expect(harness.origin.commands.contains(ENTITY)).toBe(true);
    expect(harness.host.entries.peek()[0]?.unknown).toBe(true);
    expect(harness.host.entries.peek()[0]?.error).toBe("http_409");
    expect(harness.origin.rejections.peek()).toEqual([]);

    harness.setWritesAllowed(false);
    harness.host.retry(commandId);
    await settleRealTimers();
    expect(fetch.writes).toHaveLength(2);
    expect(harness.host.entries.peek()[0]?.unknown).toBe(true);

    harness.setWritesAllowed(true);
    harness.host.retry(commandId);
    await settleRealTimers();
    expect(harness.host.entries.peek()).toEqual([]);
    expect(successCalls).toBe(0);
    expect(harness.origin.rejections.peek()[0]?.result).toEqual(err("mail_template_conflict"));
    expect(fetch.writes).toHaveLength(3);
    expect(fetch.writes.map((request) => request.method)).toEqual(["PUT", "PUT", "PUT"]);
    expect(fetch.writes.map((request) => request.url)).toEqual([URL, URL, URL]);
    expect(fetch.writes[1]?.body).toBe(fetch.writes[0]?.body);
    expect(fetch.writes[2]?.body).toBe(fetch.writes[0]?.body);
    const body = parseJson(fetch.writes[2]?.body ?? "");
    if (!body.ok) throw new Error("模板写入不是 JSON");
    expect(body.value).toEqual({
      command_id: commandId,
      expected_revision: 5,
      body: "首次确认的正文",
    });
  } finally {
    scope.dispose();
  }
});
