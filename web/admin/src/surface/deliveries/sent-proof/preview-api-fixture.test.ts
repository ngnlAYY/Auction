// Rust 的预览替身位于 HTTP 入口之前；保留它接收到的投递、证据与交付顺序。

import { expect, spyOn, test } from "bun:test";
import { err, type Result } from "@auction/web-shared/result";
import { createAdminRequestHarness } from "../../../api/api-request-fixture";
import type { previewDeliverySentProof } from "../../../api/delivery";
import { type AdminApiFailure, adminApiFailureClient } from "../../../api/failure";
import { settle } from "../delivery-dom-fixture.test";
import { requestAndPreview } from "./sent-proof-fixture";

type Request = Parameters<typeof previewDeliverySentProof>;
type Reply = Awaited<ReturnType<typeof previewDeliverySentProof>>;

/** 在与 Rust 相同的预览请求边界拦截并记录参数，由用例控制响应顺序。 */
export async function withPreviewApi(
  respond: (...request: Request) => Reply | Promise<Reply>,
  run: (requests: readonly Request[]) => Promise<void>,
): Promise<void> {
  const requests: Request[] = [];
  const replies = new Set<Promise<Reply>>();
  const stopped = Promise.withResolvers<Reply>();
  const deliveryApi = await import("../../../api/delivery");
  const spy = spyOn(deliveryApi, "previewDeliverySentProof").mockImplementation(
    async (...request) => {
      requests.push(request);
      const reply = Promise.race([
        stopped.promise,
        Promise.resolve().then(() => respond(...request)),
      ]);
      replies.add(reply);
      try {
        return await reply;
      } finally {
        replies.delete(reply);
      }
    },
  );
  try {
    await run(requests);
  } finally {
    try {
      // 用例可能在兑现可控回执之前失败。此时先收口预览任务，再恢复生产模块。
      stopped.resolve(err(adminApiFailureClient("network_error")));
      await Promise.allSettled(replies);
      await settle();
    } finally {
      spy.mockRestore();
    }
  }
}

/** 预览请求的可控回执。 */
export function deferredPreview() {
  return Promise.withResolvers<Reply>();
}

/** Rust 的过期预览错误。 */
export function stalePreview(): Result<never, AdminApiFailure> {
  return err(adminApiFailureClient("stale_delivery_state"));
}

test("预览夹具退出时结束未兑现的回执并恢复请求函数", async () => {
  const api = await import("../../../api/delivery");
  const original = api.previewDeliverySentProof;
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    harness.installFetch([]);
    const { request } = requestAndPreview("7000001", false, false);
    const args: Request = [
      host,
      request.deliveryId,
      request.evidenceReference,
      request.evidence,
      new AbortController().signal,
    ];
    const deferred = deferredPreview();
    const pending: Promise<Reply>[] = [];
    await withPreviewApi(
      () => deferred.promise,
      async (requests) => {
        pending.push(api.previewDeliverySentProof(...args));
        expect(requests).toEqual([args]);
      },
    );
    expect(await pending[0]).toEqual({ ok: false, error: { status: null, code: "network_error" } });
    expect(api.previewDeliverySentProof).toBe(original);
  } finally {
    harness.cleanup();
  }
});
