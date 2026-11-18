// 实时订阅的关闭码、HTTP 回放序列和 WebSocket 跳号补读。

import { expect, jest, spyOn, test } from "bun:test";
import { setImmediate } from "node:timers";
import { ok } from "@auction/web-shared/result";
import { flush } from "solid-js";

import type { RealtimeEvent, RealtimePage } from "./protocol";
import { subscribeAuctionRealtime } from "./subscription";
import {
  AUCTION_ID,
  mountRealtimeOwner,
  replayPage,
  SocketStub,
  serverClose,
  withSocket,
} from "./subscription-fixture";

function previewChanged(sequence: number): RealtimeEvent {
  return {
    event_id: "00000000-0000-4000-8000-000000000002",
    stream_sequence: sequence,
    kind: "preview_catalog_changed",
    schema_version: 1,
    payload: { auction_id: AUCTION_ID, world: "main", preview_revision: 2 },
    created_at: "2026-11-04T00:00:00Z",
  };
}

test.each([1008, 1013, 1001])(
  "websocket_close_preserves_the_code_and_stays_transient：关闭码 %i 传给调用方",
  async (closeCode) => {
    await withSocket(
      new CloseEvent("close", { code: closeCode }),
      (after) => replayPage([], after),
      async (after) => {
        const result = await subscribeAuctionRealtime(
          0,
          AUCTION_ID,
          "main",
          () => true,
          new AbortController().signal,
        );
        expect(after).toEqual([0]);
        expect(result).toMatchObject({ ok: false, error: { kind: "transient", closeCode } });
      },
    );
  },
);

test.each([
  [1008, "成功"],
  [1008, "在途切换目标"],
  [1013, "成功"],
  [1001, "成功"],
] as const)(
  "only_session_policy_closure_requests_an_authoritative_session_read：关闭码 %i，补读%s",
  async (closeCode, replayResult) => {
    const replayGate = Promise.withResolvers<Response>();
    let switchTarget = () => {};
    await withSocket(
      new CloseEvent("close", { code: closeCode }),
      (after, signal) => {
        if (replayResult === "在途切换目标") {
          signal?.addEventListener("abort", () => replayGate.reject(new Error("目标已切换")), {
            once: true,
          });
          queueMicrotask(switchTarget);
          return replayGate.promise;
        }
        return replayPage([], after);
      },
      async (after) => {
        const owner = mountRealtimeOwner();
        switchTarget = owner.switchTarget;
        try {
          flush();
          await Bun.sleep(0);
          expect(after).toEqual([0]);
          expect(owner.checks).toEqual(
            closeCode === 1008 && replayResult === "成功" ? ["refresh"] : [],
          );
          expect(owner.state.peek()).toBe(replayResult === "在途切换目标" ? "manual" : "degraded");
        } finally {
          owner.dispose();
          replayGate.resolve(replayPage([], 0));
        }
      },
    );
  },
);

test("websocket_close_does_not_interrupt_replay：补读完成后才处理已记录的 1008", async () => {
  const started = Promise.withResolvers<AbortSignal | null>();
  const replayGate = Promise.withResolvers<Response>();
  await withSocket(
    new CloseEvent("close", { code: 1008 }),
    (_after, signal) => {
      started.resolve(signal);
      return replayGate.promise;
    },
    async () => {
      const pages: RealtimePage[] = [];
      const subscription = subscribeAuctionRealtime(
        0,
        AUCTION_ID,
        "main",
        (page) => {
          pages.push(page);
          return true;
        },
        new AbortController().signal,
      );
      const replaySignal = await started.promise;
      try {
        expect(await Promise.race([subscription, Bun.sleep(0).then(() => "still_pending")])).toBe(
          "still_pending",
        );
        expect(replaySignal?.aborted).toBe(false);
        replayGate.resolve(replayPage([], 1));
        expect(await subscription).toMatchObject({
          ok: false,
          error: { kind: "transient", closeCode: 1008 },
        });
        expect(pages.map((page) => page.next_after)).toEqual([1]);
      } finally {
        replayGate.resolve(new Response(null, { status: 403 }));
        await subscription;
      }
    },
  );
});

test.each(["补读在途", "补读 403", "握手刚完成", "读循环报错"] as const)(
  "websocket_close_reaches_the_owner_from_each_exit：%s 收到 1008 后仅复核一次",
  async (phase) => {
    const started = Promise.withResolvers<void>();
    const replayGate = Promise.withResolvers<Response>();
    let deliverClose = () => {};
    let restoreTimeout = () => {};
    await withSocket(
      null,
      (after) => {
        started.resolve();
        if (phase === "补读在途") return replayGate.promise;
        if (phase === "补读 403") {
          const forbidden = new Response(null, { status: 403 });
          forbidden.arrayBuffer = () => {
            const clear = globalThis.clearTimeout;
            const transportFinished = spyOn(globalThis, "clearTimeout").mockImplementation(
              (handle) => {
                Reflect.apply(clear, globalThis, [handle]);
                transportFinished.mockRestore();
                // 传输层已撤掉请求定时器，再把关闭帧排到订阅收口前。
                queueMicrotask(() => queueMicrotask(deliverClose));
              },
            );
            restoreTimeout = () => transportFinished.mockRestore();
            return Promise.reject(new Error("403 正文断流"));
          };
          return forbidden;
        }
        return replayPage([], after);
      },
      async (requestedAfter, created) => {
        const owner = mountRealtimeOwner();
        try {
          flush();
          const socket = await created;
          deliverClose = () => serverClose(socket, 1008);
          expect(owner.checks).toEqual([]);
          if (phase === "握手刚完成") {
            socket.readyState = SocketStub.OPEN;
            socket.dispatchEvent(new Event("open"));
            deliverClose();
          } else {
            await started.promise;
            if (phase === "读循环报错") {
              await Bun.sleep(0);
              expect(owner.state.peek()).toBe("live");
              socket.dispatchEvent(new Event("error"));
            }
            if (phase !== "补读 403") {
              deliverClose();
            }
            if (phase === "补读在途") {
              await Bun.sleep(0);
              expect(owner.checks).toEqual([]);
              replayGate.resolve(replayPage([], 0));
            }
          }
          await Bun.sleep(0);
          expect(owner.checks).toEqual(["refresh"]);
          expect(owner.state.peek()).toBe(phase === "补读 403" ? "manual" : "degraded");
          expect(requestedAfter).toEqual(phase === "握手刚完成" ? [] : [0]);
        } finally {
          restoreTimeout();
          owner.dispose();
          replayGate.resolve(new Response(null, { status: 403 }));
        }
      },
      phase === "握手刚完成" ? SocketStub.CONNECTING : SocketStub.OPEN,
    );
  },
);

test.each(["结束后收到", "结束前已记录"] as const)(
  "socket_error_after_29_5_seconds_remains_a_failed_attempt：29.5 秒报错，关闭码%s",
  async (delivery) => {
    jest.useFakeTimers();
    const random = spyOn(Math, "random").mockReturnValue(0);
    const settle = () => new Promise<void>((resolve) => setImmediate(resolve));
    const advance = async (milliseconds: number): Promise<void> => {
      jest.advanceTimersByTime(milliseconds);
      await settle();
    };
    try {
      await withSocket(
        null,
        (after) => replayPage([], after),
        async (_after, created, sockets) => {
          const owner = mountRealtimeOwner(() => {
            if (delivery === "结束前已记录") {
              // 会话复核耗时跨过健康阈值，退避仍须使用报错时的时长。
              jest.setSystemTime(Date.now() + 1_000);
            }
          });
          try {
            flush();
            const first = await created;
            await settle();
            expect(owner.state.peek()).toBe("live");
            serverClose(first, 1001);
            await settle();
            await advance(500);
            const second = sockets[1];
            if (second === undefined) throw new Error("首次失败后没有重连");
            expect(owner.state.peek()).toBe("live");

            await advance(29_500);
            second.dispatchEvent(new Event("error"));
            if (delivery === "结束前已记录") {
              serverClose(second, 1008);
            }
            await settle();
            expect(owner.state.peek()).toBe("degraded");

            if (delivery === "结束后收到") {
              await advance(600);
              serverClose(second, 1008);
              await settle();
              expect(owner.checks).toEqual([]);
              await advance(399);
            } else {
              expect(owner.checks).toEqual(["refresh"]);
              await advance(999);
            }
            // 第二次失败的退避是 1000ms；被误判健康会按 500ms 重连。
            expect(sockets).toHaveLength(2);
            await advance(1);
            expect(sockets).toHaveLength(3);

            if (delivery === "结束后收到") {
              const third = sockets[2];
              if (third === undefined) throw new Error("第二次失败后没有重连");
              serverClose(third, 1008);
              await settle();
              expect(owner.checks).toEqual(["refresh"]);
            }
          } finally {
            owner.dispose();
          }
        },
      );
    } finally {
      random.mockRestore();
      jest.useRealTimers();
    }
  },
);

test.each([false, true])(
  "replay_returns_before_handling_the_buffered_socket_error：第 29.5 秒补读中报错，已收到关闭帧 %s",
  async (withClose) => {
    jest.useFakeTimers();
    const random = spyOn(Math, "random").mockReturnValue(0);
    const settle = () => new Promise<void>((resolve) => setImmediate(resolve));
    const advance = async (milliseconds: number): Promise<void> => {
      jest.advanceTimersByTime(milliseconds);
      await settle();
    };
    const pages = Array.from({ length: 4 }, () => Promise.withResolvers<Response>());
    let requests = 0;
    try {
      await withSocket(
        null,
        (after) => {
          requests += 1;
          return pages[requests - 2]?.promise ?? replayPage([], after);
        },
        async (requestedAfter, created, sockets) => {
          const owner = mountRealtimeOwner();
          try {
            flush();
            const first = await created;
            await settle();
            serverClose(first, 1001);
            await settle();
            await advance(500);
            const second = sockets[1];
            const lastPage = pages[3];
            if (second === undefined || lastPage === undefined) throw new Error("补读未开始");

            // 分页各自使用请求预算，第四页仍在途时连接已存活 29.5 秒。
            for (const [index, page] of pages.slice(0, 3).entries()) {
              await advance(9_500);
              page.resolve(replayPage([], index + 1, true));
              await settle();
            }
            expect(requestedAfter).toEqual([0, 0, 1, 2, 3]);
            await advance(1_000);
            second.dispatchEvent(new Event("error"));
            if (withClose) serverClose(second, 1008);
            await settle();
            expect(owner.checks).toEqual([]);
            expect(owner.state.peek()).toBe("live");

            await advance(100);
            lastPage.resolve(replayPage([], 3));
            await settle();
            expect(owner.state.peek()).toBe("degraded");
            expect(owner.checks).toEqual(withClose ? ["refresh"] : []);
            await advance(999);
            expect(sockets).toHaveLength(2);
            await advance(1);
            expect(sockets).toHaveLength(3);
          } finally {
            owner.dispose();
            for (const page of pages) page.resolve(replayPage([], 3));
          }
        },
      );
    } finally {
      random.mockRestore();
      jest.useRealTimers();
    }
  },
);

test("recorded_close_code_keeps_the_original_read_error：已记录的 1008 保留原读取错误", async () => {
  const live = Promise.withResolvers<void>();
  await withSocket(
    null,
    (after) => replayPage([], after),
    async (_after, created) => {
      const subscription = subscribeAuctionRealtime(
        0,
        AUCTION_ID,
        "main",
        () => {
          live.resolve();
          return true;
        },
        new AbortController().signal,
      );
      const socket = await created;
      await live.promise;
      socket.dispatchEvent(new Event("error"));
      serverClose(socket, 1008);
      expect(await subscription).toMatchObject({
        ok: false,
        error: {
          kind: "transient",
          closeCode: 1008,
          detail: "实时 WebSocket 读取失败：WebSocket 错误",
        },
      });
    },
  );
});

test.each([1, 2])(
  "websocket_sequence_requires_the_next_number：首个实时帧序号 %i",
  async (sequence) => {
    const frame = new MessageEvent("message", { data: JSON.stringify(previewChanged(sequence)) });
    await withSocket(
      frame,
      (after) => replayPage([], after),
      async (after) => {
        const controller = new AbortController();
        const pages: RealtimePage[] = [];
        const result = await subscribeAuctionRealtime(
          0,
          AUCTION_ID,
          "main",
          (page) => {
            pages.push(page);
            if (page.events.length > 0) controller.abort();
            return true;
          },
          controller.signal,
        );
        if (sequence === 1) {
          expect(result).toEqual(ok(null));
          expect(after).toEqual([0]);
          expect(pages.map((page) => page.next_after)).toEqual([0, 1]);
        } else {
          expect(result).toMatchObject({
            ok: false,
            error: { kind: "transient", detail: "实时事件序列出现断档", closeCode: null },
          });
          expect(after).toEqual([0, 0]);
          expect(pages.every((page) => page.events.length === 0)).toBe(true);
        }
      },
    );
  },
);

test.each([
  [2, 2],
  [3, 2],
  [2, 4],
])(
  "http_replay_rejects_invalid_sequences：HTTP 回放拒绝无效的序号 %i、%i",
  async (first, second) => {
    await withSocket(
      null,
      () => replayPage([previewChanged(first), previewChanged(second)], 3),
      async () => {
        const controller = new AbortController();
        let delivered = 0;
        const result = await subscribeAuctionRealtime(
          0,
          AUCTION_ID,
          "main",
          () => {
            delivered += 1;
            controller.abort();
            return true;
          },
          controller.signal,
        );
        expect(result).toMatchObject({ ok: false, error: { detail: "实时事件页序列无效" } });
        expect(delivered).toBe(0);
      },
    );
  },
);
