// Admin 写请求内核的预算与门禁回归：自定义超时、默认预算、同形对象与发送前后状态。

import { expect, jest, spyOn, test } from "bun:test";
import { setImmediate } from "node:timers";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { err } from "@auction/web-shared/result";

import {
  type AdminSession,
  type AdminSessionHostRegistration,
  registerAdminSessionHost,
} from "./session";
import { type PreparedJsonCommand, prepareJsonCommand, sendPreparedJsonWrite } from "./transport";

const CSRF_TOKEN = "csrf-token";
const SESSION_PATH = "/api/v1/admin/session?include=server_time";
// 内核会在交给 fetch 前用 `new Request` 复核目标；Bun 测试没有文档基准地址，这里用绝对 URL。
const WRITE_PATH =
  "https://auction.test/api/v1/admin/credentials/11111111-1111-4111-8111-111111111111/check";

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

/** 内存 fetch 替身：会话 GET 返回固定会话，业务写入交给调用方决定。 */
function installMemoryFetch(
  write: (init: RequestInit | undefined) => Promise<Response> | Response,
  session: () => Promise<Response> | Response = () => Response.json(SESSION_BODY),
): MemoryFetch {
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
          return await session();
        }
        started.resolve();
        return await write(init);
      },
      { preconnect: () => {} },
    ) as typeof fetch,
  );
  return { requests, writeStarted: started.promise, restore: () => spy.mockRestore() };
}

/** 业务写入一直挂起，直到传输层按预算或取消信号中止它。 */
function hangUntilAbort(init: RequestInit | undefined): Promise<Response> {
  return new Promise<Response>((_resolve, reject) => {
    init?.signal?.addEventListener("abort", () => reject(new Error("写请求已中止")), {
      once: true,
    });
  });
}

function registerHost(writesAllowed: () => boolean = () => true): AdminSessionHostRegistration {
  return registerAdminSessionHost({
    check() {},
    writesAllowed,
    confirmed: () => SESSION,
  });
}

function settle(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

/** 让已经发出的请求体微任务跑完，确认它此时仍未结束。 */
async function expectStillPending(done: () => boolean): Promise<void> {
  await settle();
  expect(done()).toBe(false);
}

function preparedCommand(): PreparedJsonCommand {
  const prepared = prepareJsonCommand({ command_id: "00000000-0000-4000-8000-000000000001" });
  if (!prepared.ok) {
    throw new Error("测试命令准备失败");
  }
  return prepared.value;
}

test("自定义预算决定写请求的超时时刻", async () => {
  jest.useFakeTimers();
  const host = registerHost();
  const memory = installMemoryFetch(hangUntilAbort);
  try {
    const pending = sendPreparedJsonWrite(
      host.host,
      WRITE_PATH,
      "POST",
      preparedCommand(),
      new AbortController().signal,
      3_000,
    );
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
    jest.advanceTimersByTime(2_999);
    await expectStillPending(() => done);
    jest.advanceTimersByTime(1);
    expect(await pending).toEqual(err({ status: null, code: "request_timeout" }));
    expect(memory.requests).toEqual([SESSION_PATH, WRITE_PATH]);
  } finally {
    memory.restore();
    host.release();
    jest.useRealTimers();
  }
});

test("缺省预算仍是共享层的默认值", async () => {
  jest.useFakeTimers();
  const host = registerHost();
  const memory = installMemoryFetch(hangUntilAbort);
  try {
    const pending = sendPreparedJsonWrite(
      host.host,
      WRITE_PATH,
      "POST",
      preparedCommand(),
      new AbortController().signal,
    );
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
    jest.advanceTimersByTime(DEFAULT_REQUEST_TIMEOUT_MILLIS - 1);
    await expectStillPending(() => done);
    jest.advanceTimersByTime(1);
    expect(await pending).toEqual(err({ status: null, code: "request_timeout" }));
  } finally {
    memory.restore();
    host.release();
    jest.useRealTimers();
  }
});

test("没有登记的副本仍被内核拒绝且不发出请求", async () => {
  const host = registerHost();
  const memory = installMemoryFetch(() => {
    throw new Error("不应发出请求");
  });
  try {
    const copy = { ...preparedCommand() } as PreparedJsonCommand;
    expect(
      await sendPreparedJsonWrite(
        host.host,
        WRITE_PATH,
        "POST",
        copy,
        new AbortController().signal,
      ),
    ).toEqual(err({ status: null, code: "invalid_request" }));
    expect(memory.requests).toEqual([]);
  } finally {
    memory.restore();
    host.release();
  }
});

test("发出前取消不会发送会话或业务请求", async () => {
  const host = registerHost();
  const memory = installMemoryFetch(() => {
    throw new Error("不应发出请求");
  });
  try {
    const controller = new AbortController();
    controller.abort();
    expect(
      await sendPreparedJsonWrite(
        host.host,
        WRITE_PATH,
        "POST",
        preparedCommand(),
        controller.signal,
      ),
    ).toEqual(err({ status: null, code: "not_sent:network_error" }));
    expect(memory.requests).toEqual([]);
  } finally {
    memory.restore();
    host.release();
  }
});

test("CSRF 等待期间撤回写许可时不发送业务写入", async () => {
  let allowed = true;
  const host = registerHost(() => allowed);
  const memory = installMemoryFetch(hangUntilAbort, () => {
    allowed = false;
    return Response.json(SESSION_BODY);
  });
  try {
    expect(
      await sendPreparedJsonWrite(
        host.host,
        WRITE_PATH,
        "POST",
        preparedCommand(),
        new AbortController().signal,
      ),
    ).toEqual(err({ status: null, code: "unauthenticated" }));
    expect(memory.requests).toEqual([SESSION_PATH]);
  } finally {
    memory.restore();
    host.release();
  }
});

test("返回后宿主已注销时结果未知", async () => {
  const host = registerHost();
  const memory = installMemoryFetch(() => {
    host.release();
    return Response.json({ code: "ok", message: "", data: {} });
  });
  try {
    expect(
      await sendPreparedJsonWrite(
        host.host,
        WRITE_PATH,
        "POST",
        preparedCommand(),
        new AbortController().signal,
      ),
    ).toEqual(err({ status: null, code: "network_error" }));
    expect(memory.requests).toEqual([SESSION_PATH, WRITE_PATH]);
  } finally {
    memory.restore();
    host.release();
  }
});
