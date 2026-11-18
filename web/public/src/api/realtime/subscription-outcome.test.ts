// 补读正文超时与被替换订阅的外壳回调回归测试。

import { expect, jest, spyOn, test } from "bun:test";
import { setImmediate } from "node:timers";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { flush } from "solid-js";

import { mountRealtimeOwner, replayPage, serverClose, withSocket } from "./subscription-fixture";

function settle(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

test.each([403, 200])(
  "replay_body_timeout_preserves_the_observed_http_status：%i 响应头后的正文超时",
  async (status) => {
    jest.useFakeTimers();
    const random = spyOn(Math, "random").mockReturnValue(0);
    const readingBody = Promise.withResolvers<void>();
    let firstReplay = true;
    const advance = async (milliseconds: number): Promise<void> => {
      jest.advanceTimersByTime(milliseconds);
      await settle();
    };
    try {
      await withSocket(
        null,
        (after, signal) => {
          if (!firstReplay) return replayPage([], after);
          firstReplay = false;
          const response = new Response(null, { status });
          response.arrayBuffer = () => {
            readingBody.resolve();
            return new Promise<ArrayBuffer>((_resolve, reject) => {
              signal?.addEventListener("abort", () => reject(new Error("正文已取消")), {
                once: true,
              });
            });
          };
          return response;
        },
        async (_after, created, sockets) => {
          const owner = mountRealtimeOwner();
          try {
            flush();
            const socket = await created;
            await readingBody.promise;
            if (status === 403) {
              serverClose(socket, 1008);
            }
            await settle();
            expect(owner.checks).toEqual([]);
            expect(owner.state.peek()).toBe("connecting");

            await advance(DEFAULT_REQUEST_TIMEOUT_MILLIS);
            expect(owner.checks).toEqual(status === 403 ? ["refresh"] : []);
            expect(owner.state.peek()).toBe(status === 403 ? "manual" : "degraded");
            await advance(499);
            expect(sockets).toHaveLength(1);
            await advance(1);
            expect(sockets).toHaveLength(status === 403 ? 1 : 2);
          } finally {
            owner.dispose();
            await settle();
          }
        },
      );
    } finally {
      random.mockRestore();
      jest.useRealTimers();
    }
  },
);

test("replaced_subscription_does_not_request_a_session_check：收到 1008 后切换目标，旧订阅不复核会话", async () => {
  await withSocket(
    null,
    (after) => replayPage([], after),
    async (_after, created) => {
      const owner = mountRealtimeOwner();
      try {
        flush();
        const socket = await created;
        await settle();
        expect(owner.state.peek()).toBe("live");

        serverClose(socket, 1008);
        // 读循环已排队处理关闭帧，外壳接收旧订阅结果前切换目标。
        queueMicrotask(owner.switchTarget);
        await settle();
        expect(owner.state.peek()).toBe("manual");
        expect(owner.checks).toHaveLength(0);
      } finally {
        owner.dispose();
      }
    },
  );
});
