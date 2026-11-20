// 凭据检查与导入的写入预算回归：各自的超时来自 Rust 对应入口，并经由内核统一发送。

import { afterAll, expect, jest, spyOn, test } from "bun:test";
import { setImmediate } from "node:timers";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { err } from "@auction/web-shared/result";

import {
  type AdminSession,
  type AdminSessionHost,
  type AdminSessionHostRegistration,
  registerAdminSessionHost,
} from "../session";
import {
  type AdminCredential,
  checkAdminCredential,
  createAdminCredential,
  prepareAdminCredentialCreate,
} from "./index";

const CREDENTIAL_ID = "11111111-1111-4111-8111-111111111111";
const COMMAND_ID = "00000000-0000-4000-8000-000000000002";
const CSRF_TOKEN = "csrf-token";
const SESSION_PATH = "/api/v1/admin/session?include=server_time";
const BROWSER_BASE = "https://auction.test";

// Bun 测试没有文档基准地址，而 Admin 的接口层用相对 URL；补上基准后再构造 Request。
const realRequest = globalThis.Request;
globalThis.Request = new Proxy(realRequest, {
  construct(target, args) {
    const input = args[0] as RequestInfo | URL;
    const init = args[1] as RequestInit | undefined;
    const resolved = typeof input === "string" ? new URL(input, BROWSER_BASE).href : input;
    return new target(resolved, init);
  },
}) as typeof Request;
afterAll(() => {
  globalThis.Request = realRequest;
});

const SESSION: AdminSession = Object.freeze({
  actor: "admin",
  csrfToken: CSRF_TOKEN,
  csrf_token: CSRF_TOKEN,
  expiresAt: 1n,
  absoluteExpiresAt: 2n,
  serverTime: 0n,
  receivedAtMillis: 0,
});

const SESSION_BODY = {
  code: "ok",
  message: "",
  data: {
    authenticated: true,
    actor: SESSION.actor,
    csrf_token: CSRF_TOKEN,
    expires_at: "2026-11-19T01:00:00Z",
    absolute_expires_at: "2026-11-20T00:00:00Z",
    server_time: "2026-11-19T00:00:00Z",
  },
};

interface MemoryFetch {
  readonly requests: string[];
  readonly writeStarted: Promise<void>;
  restore(): void;
}

/** 内存 fetch 替身：会话 GET 直接成功，业务写入挂起到超时或取消。 */
function installMemoryFetch(): MemoryFetch {
  const requests: string[] = [];
  const started = Promise.withResolvers<void>();
  const spy = spyOn(globalThis, "fetch");
  spy.mockImplementation(
    Object.assign(
      async (input: URL | RequestInfo, init?: RequestInit): Promise<Response> => {
        const url =
          input instanceof URL ? input.href : typeof input === "string" ? input : input.url;
        requests.push(url);
        if (url.startsWith(SESSION_PATH)) {
          return Response.json(SESSION_BODY);
        }
        started.resolve();
        return await new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => reject(new Error("写请求已中止")), {
            once: true,
          });
        });
      },
      { preconnect: () => {} },
    ) as typeof fetch,
  );
  return { requests, writeStarted: started.promise, restore: () => spy.mockRestore() };
}

function registerHost(): AdminSessionHostRegistration {
  return registerAdminSessionHost({
    check() {},
    writesAllowed: () => true,
    confirmed: () => SESSION,
  });
}

function settle(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

function credential(kind: string, world: string): AdminCredential {
  return {
    id: CREDENTIAL_ID,
    kind,
    world,
    label: "Worker",
    player_name: null,
    status: "active",
    state_version: 7,
    created_at: "2026-11-18T00:00:00Z",
    rotated_at: null,
    updated_at: "2026-11-18T00:00:00Z",
  };
}

/** 在假定时器下确认写请求恰好在预算毫秒触发超时，并返回 Rust 的错误码。 */
async function expectWriteBudget(
  start: (host: AdminSessionHost, signal: AbortSignal) => Promise<unknown>,
  budget: number,
  expectedPath: string,
): Promise<void> {
  jest.useFakeTimers();
  const registration = registerHost();
  const memory = installMemoryFetch();
  try {
    const pending = start(registration.host, new AbortController().signal);
    let done = false;
    void pending.then(
      () => {
        done = true;
      },
      () => {
        done = true;
      },
    );
    await memory.writeStarted;
    jest.advanceTimersByTime(budget - 1);
    await settle();
    expect(done).toBe(false);
    jest.advanceTimersByTime(1);
    await settle();
    if (!done) {
      throw new Error(`写请求未在 ${budget} 毫秒触发超时`);
    }
    expect(await pending).toEqual(err("request_timeout"));
    expect(memory.requests).toEqual([SESSION_PATH, expectedPath]);
  } finally {
    memory.restore();
    registration.release();
    jest.useRealTimers();
  }
}

test("shared 邮件检查使用 Rust 的 18 秒预算", async () => {
  await expectWriteBudget(
    (host, signal) => checkAdminCredential(host, credential("mail_cookie", "shared"), signal),
    18_000,
    `/api/v1/admin/credentials/${CREDENTIAL_ID}/check`,
  );
});

test("其他凭据检查沿用默认预算", async () => {
  await expectWriteBudget(
    (host, signal) => checkAdminCredential(host, credential("mail_cookie", "main"), signal),
    DEFAULT_REQUEST_TIMEOUT_MILLIS,
    `/api/v1/admin/credentials/${CREDENTIAL_ID}/check`,
  );
});

test("凭据导入使用 Rust 的 20 秒预算", async () => {
  const prepared = prepareAdminCredentialCreate({
    command_id: COMMAND_ID,
    kind: "mail_cookie",
    world: "main",
    label: "Main",
    secret: "ck",
  });
  if (!prepared.ok) {
    throw new Error("测试导入命令准备失败");
  }
  await expectWriteBudget(
    (host, signal) => createAdminCredential(host, prepared, signal),
    20_000,
    "/api/v1/admin/credentials",
  );
});
