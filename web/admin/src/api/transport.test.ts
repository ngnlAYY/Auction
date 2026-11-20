// Admin 写请求内核的预算与门禁回归：自定义超时、默认预算、同形对象与发送前后状态。

import { expect, jest, spyOn, test } from "bun:test";
import { setImmediate } from "node:timers";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { err } from "@auction/web-shared/result";

import {
  type AdminWriteFailure,
  adminWriteFailureDisplayCode,
  classifyAdminWriteFailure,
  observedResponseStatus,
} from "./failure";
import { runHistoryCase } from "./failure-history-fixture.test";
import {
  type AdminSession,
  type AdminSessionHostRegistration,
  registerAdminSessionHost,
} from "./session";
import {
  type AdminWriteOperation,
  type PreparedJsonCommand,
  prepareJsonCommand,
  sendPreparedJsonWrite,
} from "./transport";

const CSRF_TOKEN = "csrf-token";
const SESSION_PATH = "/api/v1/admin/session?include=server_time";
// 内核会在交给 fetch 前用 `new Request` 复核目标；Bun 测试没有文档基准地址，这里用绝对 URL。
const WRITE_PATH =
  "https://auction.test/api/v1/admin/credentials/11111111-1111-4111-8111-111111111111/check";

const WRITE_OPERATION: AdminWriteOperation = {
  url: WRITE_PATH,
  method: "POST",
  endpoint: "general",
};

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
      WRITE_OPERATION,
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
    expect(await pending).toEqual(
      err({ kind: "outcome-unknown", code: "request_timeout", responseStatus: null }),
    );
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
      WRITE_OPERATION,
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
    expect(await pending).toEqual(
      err({ kind: "outcome-unknown", code: "request_timeout", responseStatus: null }),
    );
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
      await sendPreparedJsonWrite(host.host, WRITE_OPERATION, copy, new AbortController().signal),
    ).toEqual(err({ kind: "not-sent", code: "invalid_request" }));
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
      await sendPreparedJsonWrite(host.host, WRITE_OPERATION, preparedCommand(), controller.signal),
    ).toEqual(err({ kind: "not-sent", code: "network_error" }));
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
        WRITE_OPERATION,
        preparedCommand(),
        new AbortController().signal,
      ),
    ).toEqual(err({ kind: "not-sent", code: "unauthenticated" }));
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
        WRITE_OPERATION,
        preparedCommand(),
        new AbortController().signal,
      ),
    ).toEqual(err({ kind: "outcome-unknown", code: "network_error", responseStatus: null }));
    expect(memory.requests).toEqual([SESSION_PATH, WRITE_PATH]);
  } finally {
    memory.restore();
    host.release();
  }
});

test("只从完整收到的畸形正文观察 HTTP 409，两种失败均保留未决", async () => {
  const incompleteJson = '{"code":"alert_stale"';
  let sentChunk = false;
  const truncatedBody = new ReadableStream<Uint8Array>(
    {
      pull(controller) {
        if (sentChunk) {
          controller.error(new Error("响应正文传输中断"));
          return;
        }
        sentChunk = true;
        controller.enqueue(new TextEncoder().encode(incompleteJson));
      },
    },
    { highWaterMark: 0 },
  );
  const cases = [
    ["完整收到的畸形正文", new Response(incompleteJson, { status: 409 }), 409, "http_409"],
    ["中途失败的正文流", new Response(truncatedBody, { status: 409 }), null, "network_error"],
  ] as const;
  const host = registerHost();
  try {
    for (const [label, response, status, display] of cases) {
      const memory = installMemoryFetch(() => response);
      try {
        const result = await sendPreparedJsonWrite(
          host.host,
          WRITE_OPERATION,
          preparedCommand(),
          new AbortController().signal,
        );
        expect(memory.requests, label).toEqual([SESSION_PATH, WRITE_PATH]);
        if (result.ok) throw new Error(`异常正文被当成成功：${label}`);
        expect(result.error.kind, label).toBe("outcome-unknown");
        expect(observedResponseStatus(result.error), label).toBe(status);
        await runHistoryCase(result.error, { display, scope: "unknown" });
      } finally {
        memory.restore();
      }
    }
  } finally {
    host.release();
  }
});

test("宿主只采信本发送器登记的原始写回执", async () => {
  // Bun 用查询串单独求值模块，复用原会话和解码依赖，产生独立的回执登记表。
  type Transport = typeof import("./transport");
  // @ts-expect-error TypeScript 不解析 Bun 的模块查询串，类型由原模块提供。
  const isolated: Transport = await import("./transport?isolated-write-receipts");
  const host = registerHost();
  const memory = installMemoryFetch(() =>
    Response.json(
      { code: "mail_template_conflict", message: "模板版本已变更", data: null, request_id: null },
      { status: 409 },
    ),
  );
  try {
    const signal = new AbortController().signal;
    const original = await sendPreparedJsonWrite(
      host.host,
      WRITE_OPERATION,
      preparedCommand(),
      signal,
    );
    const preparedElsewhere = isolated.prepareJsonCommand({
      command_id: "00000000-0000-4000-8000-000000000001",
    });
    if (!preparedElsewhere.ok) throw new Error("独立发送器未能准备命令");
    const foreign = await isolated.sendPreparedJsonWrite(
      host.host,
      WRITE_OPERATION,
      preparedElsewhere.value,
      signal,
    );
    expect(memory.requests).toEqual([SESSION_PATH, WRITE_PATH, SESSION_PATH, WRITE_PATH]);
    if (original.ok || original.error.kind !== "verified-http") {
      throw new Error("原发送器未返回可裁定的回执");
    }
    if (foreign.ok || foreign.error.kind !== "verified-http") {
      throw new Error("独立发送器未返回自己的回执");
    }

    expect(observedResponseStatus(original.error)).toBe(409);
    await runHistoryCase(original.error, {
      display: "mail_template_conflict",
      scope: "replay_checked",
    });
    const untrustedCases = [
      [
        "未登记的同形对象",
        {
          ...original.error.receipt,
          status: 409,
          endpoint: "general",
          protocolCode: "mail_template_conflict",
        },
      ],
      ["展开副本", { ...original.error.receipt }],
      ["结构化克隆", structuredClone(original.error.receipt)],
      ["另一发送器的原始回执", foreign.error.receipt],
    ] as const;
    for (const [label, receipt] of untrustedCases) {
      const failure: AdminWriteFailure = { kind: "verified-http", receipt };
      expect(classifyAdminWriteFailure(failure), label).toBe("unknown");
      expect(adminWriteFailureDisplayCode(failure), label).toBe("invalid_response");
      expect(observedResponseStatus(failure), label).toBeNull();
      await runHistoryCase(failure, { display: "invalid_response", scope: "unknown" });
    }
  } finally {
    memory.restore();
    host.release();
  }
});
