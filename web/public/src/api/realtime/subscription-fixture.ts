// 实时订阅测试共用的 HTTP、WebSocket 和外壳替身。

import { createPageVisibility } from "@auction/web-shared/platform/visibility";
import { createCell } from "@auction/web-shared/reactive/cell";
import { children, createEffect, createRoot, flush } from "solid-js";

import { PublicRealtimeOwner, type PublicRealtimeTarget } from "../../surface/board/realtime";
import { PublicBidRefreshContext } from "../../surface/board/refresh";
import type { LiveConnectionState } from "../../surface/query";
import { PageVisibilityContext } from "../../surface/shell/contexts";
import { registerPublicSessionCheckTrigger, type SessionRefreshReason } from "../session";
import type { RealtimeEvent } from "./protocol";

export const AUCTION_ID = "00000000-0000-4000-8000-000000000001";

/** 构造默认无需再翻页的回放响应。 */
export function replayPage(
  events: readonly RealtimeEvent[],
  cursor: number,
  hasMore = false,
): Response {
  return Response.json({
    code: "ok",
    message: "ok",
    data: {
      auction_id: AUCTION_ID,
      events,
      next_after: cursor,
      has_more: hasMore,
      refetch_required: false,
    },
  });
}

/** 套接字替身的就绪状态由测试推进。 */
export class SocketStub extends EventTarget {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  readyState = SocketStub.OPEN;

  close(): void {
    // 模拟发起关闭。实际 CloseEvent 由服务端替身另行交付。
    this.readyState = SocketStub.CLOSING;
  }
}

/** 由服务端替身交付关闭事件。 */
export function serverClose(socket: SocketStub, code: number): void {
  socket.readyState = SocketStub.CLOSED;
  socket.dispatchEvent(new CloseEvent("close", { code }));
}

/** 在用例期间替换浏览器套接字与回放请求，结束后恢复原全局属性。 */
export async function withSocket(
  fromServer: Event | null,
  replay: (after: number, signal: AbortSignal | null) => Response | Promise<Response>,
  run: (
    requestedAfter: number[],
    created: Promise<SocketStub>,
    sockets: SocketStub[],
  ) => Promise<void>,
  initialState = SocketStub.OPEN,
): Promise<void> {
  let resolveCreated: ((socket: SocketStub) => void) | null = null;
  const created = new Promise<SocketStub>((resolve) => {
    resolveCreated = resolve;
  });
  const sockets: SocketStub[] = [];
  class TrackedSocket extends SocketStub {
    constructor() {
      super();
      this.readyState = initialState;
      sockets.push(this);
      resolveCreated?.(this);
    }
  }
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window") ?? null;
  const originalSocket = Object.getOwnPropertyDescriptor(globalThis, "WebSocket") ?? null;
  const originalFetch = Object.getOwnPropertyDescriptor(globalThis, "fetch") ?? null;
  const requestedAfter: number[] = [];
  const fetchStub = Object.assign(
    async (input: URL | RequestInfo, init?: RequestInit) => {
      if (typeof input !== "string" || !input.startsWith("/api/v1/events?")) {
        return new Response(null, { status: 204 });
      }
      const after = Number(new URL(input, "https://example.test").searchParams.get("after"));
      requestedAfter.push(after);
      const response = replay(after, init?.signal ?? null);
      if (requestedAfter.length === 1 && fromServer !== null) {
        sockets[0]?.dispatchEvent(fromServer);
      }
      return response;
    },
    { preconnect: () => {} },
  );
  try {
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: { location: new URL("https://example.test") },
    });
    Object.defineProperty(globalThis, "WebSocket", { configurable: true, value: TrackedSocket });
    Object.defineProperty(globalThis, "fetch", { configurable: true, value: fetchStub });
    await run(requestedAfter, created, sockets);
  } finally {
    restoreGlobal("fetch", originalFetch);
    restoreGlobal("WebSocket", originalSocket);
    restoreGlobal("window", originalWindow);
  }
}

function restoreGlobal(name: string, descriptor: PropertyDescriptor | null): void {
  if (descriptor === null) {
    Reflect.deleteProperty(globalThis, name);
  } else {
    Object.defineProperty(globalThis, name, descriptor);
  }
}

/** 挂载看板订阅和会话入口，供测试读取连接状态和复核原因。 */
export function mountRealtimeOwner(onSessionCheck: () => void = () => {}) {
  const checks: SessionRefreshReason[] = [];
  const unregister = registerPublicSessionCheckTrigger((reason) => {
    checks.push(reason);
    onSessionCheck();
  });
  const state = createCell<LiveConnectionState>("connecting");
  const target = createCell<PublicRealtimeTarget>({
    kind: "auction",
    world: "main",
    auctionId: AUCTION_ID,
    previewRevision: 0,
  });
  const dispose = createRoot((disposeRoot) => {
    const owner = children(() =>
      PageVisibilityContext({
        value: createPageVisibility(),
        get children() {
          return PublicBidRefreshContext({
            value: { revision: createCell(0), notify: () => {} },
            get children() {
              return PublicRealtimeOwner({
                get target() {
                  return target();
                },
                userId: "user",
                refreshGeneration: createCell(0),
                auctionAfter: createCell<readonly [string, number] | null>(null),
                realtimeState: state,
              });
            },
          });
        },
      }),
    );
    createEffect(owner, () => {});
    return disposeRoot;
  });
  return {
    checks,
    state,
    switchTarget: () => {
      target.set({ kind: "inactive", world: "main", preview: false });
      flush();
    },
    dispose: () => {
      dispose();
      unregister();
    },
  };
}
