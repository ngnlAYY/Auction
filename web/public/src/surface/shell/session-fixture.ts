// 会话回归共用的 Cookie、Web Lock、存储与服务端替身。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createEffect, createRoot, flush } from "solid-js";

import { PUBLIC_SESSION_CHANGE_STORAGE_KEY, type PublicSession } from "../../api/session";
import { invalidate } from "../../api/session/cookie";
import { resetStoreForTest } from "../../api/swr-cache";
import { createPublicShellSession } from "./session";

const SESSION_PATH = "/api/v1/session";
const LOCK_NAME = "hv.public.session-cookie";
/** 默认已登录的替身账号。 */
export const ALICE: PublicSession = {
  authenticated: true,
  user_id: "00000000-0000-4000-8000-000000000001",
  username: "Alice",
  expires_at: null,
  absolute_expires_at: null,
};
/** 切换账号的目标身份。 */
export const BOB: PublicSession = {
  ...ALICE,
  user_id: "00000000-0000-4000-8000-000000000002",
  username: "Bob",
};

/** 构造可经真实 API 解码的成功回执。 */
export function success(data: unknown): Response {
  return Response.json({ code: "ok", message: "", data });
}

/** 将会话身份转换为登录接口的回执。 */
export function loginReceipt({
  user_id,
  username,
  expires_at,
  absolute_expires_at,
}: PublicSession) {
  return success({ user_id, username, status: "active", expires_at, absolute_expires_at });
}

/** 构造 HTTP 错误回执。 */
export function reject(code: string, status: number): Response {
  return Response.json({ code, message: "", data: null }, { status });
}

/** 构造已有响应头、正文读取失败的请求结果。 */
export function brokenBody(status = 200): Response {
  return new Response(
    new ReadableStream({
      start(controller) {
        controller.error(new Error("正文中断"));
      },
    }),
    { status, headers: { "content-type": "application/json" } },
  );
}

function replaceGlobal(name: "window" | "navigator" | "fetch", value: unknown) {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, name);
  Object.defineProperty(globalThis, name, { configurable: true, value });
  return () => {
    if (descriptor === undefined) {
      Reflect.deleteProperty(globalThis, name);
    } else {
      Object.defineProperty(globalThis, name, descriptor);
    }
  };
}

function fakeBrowser(locksAvailable: boolean) {
  const events = new EventTarget();
  const storage = new Map<string, string>();
  const writes: string[] = [];
  let queue = Promise.resolve();
  let nextLock = 0;
  let held: number | null = null;
  const request = (
    name: string,
    options: { signal?: AbortSignal },
    callback: (lock: { name: string }) => Promise<void>,
  ) => {
    if (name !== LOCK_NAME) {
      throw new Error("会话请求使用了其他锁");
    }
    const granted = queue.then(async () => {
      options.signal?.throwIfAborted();
      held = ++nextLock;
      try {
        await callback({ name });
      } finally {
        held = null;
      }
    });
    queue = granted.catch(() => {
      // 取消排队项不能让后续锁请求一并失败。
    });
    return granted;
  };
  const restoreWindow = replaceGlobal(
    "window",
    Object.assign(events, {
      isSecureContext: true,
      localStorage: {
        get length() {
          return storage.size;
        },
        key: (index: number) => [...storage.keys()][index] ?? null,
        removeItem: (key: string) => storage.delete(key),
        getItem: (key: string) => storage.get(key) ?? null,
        setItem: (key: string, value: string) => {
          storage.set(key, value);
          writes.push(key);
        },
      },
    }),
  );
  const restoreNavigator = replaceGlobal("navigator", locksAvailable ? { locks: { request } } : {});
  return {
    storage,
    writes,
    currentLock: () => held,
    hold: (block: Promise<void>) => request(LOCK_NAME, {}, async () => block),
    focus: () => events.dispatchEvent(new Event("focus")),
    notifySessionRevision: () =>
      events.dispatchEvent(
        Object.assign(new Event("storage"), {
          key: PUBLIC_SESSION_CHANGE_STORAGE_KEY,
          newValue: storage.get(PUBLIC_SESSION_CHANGE_STORAGE_KEY) ?? null,
        }),
      ),
    dispose() {
      restoreNavigator();
      restoreWindow();
    },
  };
}

/** 挂载真实会话外壳，离线记录请求、锁与身份变更。 */
export function fixture(
  initial: PublicSession | null,
  options: { waitForLock?: Promise<void>; locksAvailable?: boolean } = {},
) {
  const browser = fakeBrowser(options.locksAvailable ?? true);
  const priorLock = options.waitForLock === undefined ? null : browser.hold(options.waitForLock);
  invalidate();
  resetStoreForTest();
  const signal = new AbortController().signal;
  const requests: {
    path: string;
    method: string;
    lock: number | null;
    expectedUser: string | null;
  }[] = [];
  const server = {
    identity: initial,
    read: (): Response | Promise<Response> => success(server.identity),
    synchronize: (): Response | Promise<Response> => success(server.identity),
    mutate: (_path: string, method: string, _init?: RequestInit): Response | Promise<Response> => {
      throw new Error(`意外写请求 ${method}`);
    },
  };
  const restoreFetch = replaceGlobal(
    "fetch",
    Object.assign(
      async (url: URL | RequestInfo, init?: RequestInit): Promise<Response> => {
        const path = String(url);
        const method = init?.method ?? "GET";
        const expectedUser = new Headers(init?.headers).get(PUBLIC_EXPECTED_USER_ID_HEADER);
        requests.push({ path, method, expectedUser, lock: browser.currentLock() });
        if (path.startsWith("/api/v1/auctions/current?")) {
          return success({ auction: null });
        }
        if (path === SESSION_PATH && method === "GET") {
          return server.read();
        }
        if (path === SESSION_PATH && method === "PUT") {
          if (expectedUser !== (server.identity?.user_id ?? null)) {
            throw new Error("Cookie 同步未使用锁内身份");
          }
          return server.synchronize();
        }
        return server.mutate(path, method, init);
      },
      { preconnect: () => {} },
    ),
  );
  const refreshGeneration = createCell(0);
  const shell = createRoot((dispose) => {
    const shellSession = createPublicShellSession(refreshGeneration);
    const owners: (string | null)[] = [];
    createEffect(shellSession.commandOwner, (userId) => {
      owners.push(userId);
    });
    flush();
    return { ...shellSession, owners, dispose };
  });
  return {
    ...shell,
    browser,
    server,
    requests,
    refreshGeneration,
    signal,
    priorLock,
    dispose() {
      shell.dispose();
      restoreFetch();
      browser.dispose();
      invalidate();
      resetStoreForTest();
    },
  };
}
