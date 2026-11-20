// Admin 接口测试的内存请求、认证宿主及契约样本读写入口。

import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { parseJson } from "@auction/web-shared/net/wire";
import { cleanupScope, preserveProperty } from "../test-scope-fixture";

import { type AdminWriteEndpoint, type AdminWriteFailure, observedResponseStatus } from "./failure";
import {
  type AdminSession,
  type AdminSessionHostRegistration,
  registerAdminSessionHost,
} from "./session";
import { prepareJsonCommand, sendPreparedJsonWrite } from "./transport";

const BROWSER_BASE = "https://auction.test";
const CSRF_TOKEN = "csrf-token";

/** 测试会话与 CSRF 代际；写命令的会话复核会读取这两项。 */
export const TEST_SESSION: AdminSession = Object.freeze({
  actor: "admin",
  csrfToken: CSRF_TOKEN,
  csrf_token: CSRF_TOKEN,
  expiresAt: 1n,
  absoluteExpiresAt: 2n,
  serverTime: 0n,
  receivedAtMillis: 0,
});

/** 会话 GET 的完整回执；写请求前的 CSRF 复核按它的 token 放行。 */
export const TEST_SESSION_BODY = Object.freeze({
  code: "ok",
  message: "",
  data: {
    authenticated: true,
    actor: TEST_SESSION.actor,
    csrf_token: CSRF_TOKEN,
    expires_at: "2026-11-19T01:00:00Z",
    absolute_expires_at: "2026-11-20T00:00:00Z",
    server_time: "2026-11-19T00:00:00Z",
  },
});

/** 一次 fetch 的内存回执；状态缺省为 200。 */
export interface Reply {
  readonly status?: number;
  readonly body: string;
}

/** 测试观察到的请求；正文只保留字符串形态（命令全部是 JSON 文本）。 */
export interface RecordedRequest {
  readonly url: string;
  readonly method: string;
  readonly body: string;
}

/** 模拟浏览器按同源基址构造 Request；还原时保留原属性描述符。 */
export function installBrowserRequest(): () => void {
  const restore = preserveProperty(globalThis, "Request");
  const original = globalThis.Request;
  class BrowserRequest extends original {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(typeof input === "string" ? new URL(input, BROWSER_BASE) : input, init);
    }
  }
  globalThis.Request = BrowserRequest;
  return restore;
}

/** 记录原始请求地址与正文，URL 规范化留给需要比较地址的用例。 */
export function recordRequest(input: RequestInfo | URL, init?: RequestInit): RecordedRequest {
  return {
    url: input instanceof Request ? input.url : String(input),
    method: init?.method ?? (input instanceof Request ? input.method : "GET"),
    body: typeof init?.body === "string" ? init.body : "",
  };
}

/** 测试处理器接收生产传输层提供的参数并返回内存应答。 */
export type FetchHandler = (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>;

/** 为内存处理器补上 Bun 的 fetch 接口；意外的预连接立即失败。 */
export function fixtureFetch(handler: FetchHandler): typeof fetch {
  return Object.assign((...args: Parameters<typeof fetch>) => handler(...args), {
    preconnect() {
      throw new Error("内存请求替身不允许预连接");
    },
  });
}

/** 在用例内安装内存请求，退出时按原属性描述符恢复。 */
export function installFixtureFetch(handler: FetchHandler): () => void {
  const restore = preserveProperty(globalThis, "fetch");
  globalThis.fetch = fixtureFetch(handler);
  return restore;
}

/** 每个测试文件自己的替身生命周期；用例结束后由 afterEach 统一恢复。 */
export interface AdminRequestHarness {
  /** 让测试里的相对 URL 能构造出浏览器 Request；写请求与会话 GET 都经过它。 */
  installBrowserRequest(): void;
  /** 按顺序返回内存回执并记录实际请求；多余请求直接失败。 */
  installFetch(replies: readonly Reply[]): RecordedRequest[];
  /** 登记一个写许可与已确认会话都就绪的宿主。 */
  registerHost(): AdminSessionHostRegistration;
  /** 逆序恢复本文件安装的替身与宿主登记。 */
  cleanup(): void;
}

/** 创建一个用例级的替身集合；不在模块加载时替换任何全局对象。 */
export function createAdminRequestHarness(): AdminRequestHarness {
  const scope = cleanupScope();
  return {
    installBrowserRequest() {
      scope.defer(installBrowserRequest());
    },
    installFetch(replies) {
      const queue = [...replies];
      const requests: RecordedRequest[] = [];
      scope.defer(
        installFixtureFetch(async (input, init) => {
          const request = recordRequest(input, init);
          requests.push(request);
          const reply = queue.shift();
          if (reply === undefined) {
            throw new Error(`未预期的请求 ${request.url}`);
          }
          return new Response(reply.body, { status: reply.status ?? 200 });
        }),
      );
      return requests;
    },
    registerHost() {
      const registration = registerAdminSessionHost({
        check() {},
        writesAllowed: () => true,
        confirmed: () => TEST_SESSION,
      });
      scope.defer(registration.release);
      return registration;
    },
    cleanup: scope.dispose,
  };
}

const WRITE_SESSION_BODY = {
  code: "ok",
  message: "",
  data: {
    authenticated: true,
    actor: "fixture",
    csrf_token: "csrf-fixture",
    expires_at: "2026-11-19T01:00:00Z",
    absolute_expires_at: "2026-11-20T00:00:00Z",
    server_time: "2026-11-19T00:00:00Z",
  },
};

/** 一次实际发出的写请求；正文按字符串原样保留，供测试核对冻结字节。 */
export interface CapturedWrite {
  readonly method: string;
  readonly path: string;
  readonly url: string;
  readonly body: string;
}

/** 写请求应答函数；序号从 0 开始，只统计业务写入，不包含会话复核 GET。 */
export type WriteReply = (
  request: CapturedWrite,
  init: RequestInit | undefined,
  index: number,
) => Response | Promise<Response>;

/** 为页面测试保留会话复核行为，并记录其余请求的原始正文。 */
export function installWriteFetch(reply: WriteReply): {
  readonly writes: CapturedWrite[];
  restore(): void;
} {
  const writes: CapturedWrite[] = [];
  const restore = installFixtureFetch(async (input, init) => {
    const recorded = recordRequest(input, init);
    const url = new URL(recorded.url, BROWSER_BASE);
    if (url.pathname === "/api/v1/admin/session") {
      return Response.json(WRITE_SESSION_BODY);
    }
    const request = { ...recorded, path: url.pathname, url: url.href };
    writes.push(request);
    return reply(request, init, writes.length - 1);
  });
  return { writes, restore };
}

/** 用共享 `parseJson` 解析样本文本，并按运行期检查收窄成普通对象。 */
export function parseJsonObject(text: string, label: string): Record<string, unknown> {
  const parsed = parseJson(text);
  if (!parsed.ok) {
    throw new Error(`${label} 不是合法 JSON`);
  }
  return jsonObject(parsed.value, label);
}

/** 普通 JSON 对象：非 null、非数组的 `typeof "object"`；作为后续收窄的运行期依据。 */
export function jsonObject(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} 必须是对象`);
  }
  return value as Record<string, unknown>;
}

/** 按 `/a/b/0` 路径读取；路径不存在时返回 `undefined`。 */
export function valueAt(root: unknown, path: string): unknown {
  let current = root;
  for (const key of pathParts(path)) {
    if (Array.isArray(current)) {
      current = current[Number(key)];
    } else if (typeof current === "object" && current !== null) {
      current = (current as Record<string, unknown>)[key];
    } else {
      return undefined;
    }
  }
  return current;
}

/** 按路径读取并检查一个普通 JSON 对象。 */
export function jsonObjectAt(root: unknown, path: string): Record<string, unknown> {
  return jsonObject(valueAt(root, path), path === "" ? "根对象" : path);
}

/** 按 `/a/b` 路径写入已有的父对象或数组槽位；父级不存在时失败。 */
export function setAt(root: unknown, path: string, value: unknown): void {
  const parts = pathParts(path);
  const key = parts.pop();
  if (key === undefined) {
    throw new Error("缺少字段路径");
  }
  const parent = valueAt(root, `/${parts.join("/")}`);
  if (Array.isArray(parent)) {
    parent[Number(key)] = value;
  } else if (typeof parent === "object" && parent !== null) {
    (parent as Record<string, unknown>)[key] = value;
  } else {
    throw new Error(`字段路径不存在 ${path}`);
  }
}

/** 删除对象上的一个键；与 Rust 的 `checked` 助手一样要求键确实存在。 */
export function removeAt(root: unknown, path: string): unknown {
  const parts = pathParts(path);
  const key = parts.pop();
  if (key === undefined) {
    throw new Error("缺少字段路径");
  }
  const parent = jsonObject(valueAt(root, `/${parts.join("/")}`), path);
  if (!Object.hasOwn(parent, key)) {
    throw new Error(`字段不存在 ${path}`);
  }
  const removed = parent[key];
  delete parent[key];
  return removed;
}

function pathParts(path: string): string[] {
  return path.split("/").filter((part) => part !== "");
}

/** 把测试里构造的普通 JSON 值收窄为接口参数类型；每一项都经过运行期判定。 */
export function asAdminJsonValue(value: unknown): AdminJsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new Error("JSON 数值必须有限");
    }
    return value;
  }
  if (Array.isArray(value)) {
    return value.map(asAdminJsonValue);
  }
  const object = jsonObject(value, "JSON 值");
  const result: Record<string, AdminJsonValue> = {};
  for (const [key, item] of Object.entries(object)) {
    result[key] = asAdminJsonValue(item);
  }
  return result;
}

/** 让真实写入发送器处理内存回执，保留原始状态、正文及端点证据。 */
export async function writeFailureFromReply(
  status: number,
  body: string,
  endpoint: AdminWriteEndpoint = "general",
): Promise<AdminWriteFailure> {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    harness.installFetch([{ body: JSON.stringify(TEST_SESSION_BODY) }, { status, body }]);
    const { host } = harness.registerHost();
    const prepared = prepareJsonCommand({});
    if (!prepared.ok) throw new Error("测试命令准备失败");
    const result = await sendPreparedJsonWrite(
      host,
      { url: "/api/v1/admin/test-write", method: "POST", endpoint },
      prepared.value,
      new AbortController().signal,
    );
    if (result.ok || observedResponseStatus(result.error) !== status) {
      throw new Error("测试写入未收到预期的 HTTP 失败");
    }
    return result.error;
  } finally {
    harness.cleanup();
  }
}
