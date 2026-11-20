// 认证测试的受控传输：各端点独立排队，取消信号必须让 fetch 拒绝。

import { preserveProperty } from "../../test-scope-fixture";

export { settleAuth as settle } from "./auth-environment-fixture";

/** 受控传输收到的 HTTP 回执或断线。 */
export type FetchReply =
  | {
      readonly kind: "response";
      readonly status: number;
      readonly body: string;
      readonly headers?: Record<string, string>;
    }
  | { readonly kind: "network" };

/** 可观测取消与排队状态的单端点替身。 */
export class Stub {
  calls = 0;
  cancelled = 0;
  private readonly pending: ((reply: FetchReply) => void)[] = [];

  request(signal?: AbortSignal): Promise<FetchReply> {
    this.calls += 1;
    if (signal?.aborted === true) {
      this.cancelled += 1;
      return Promise.reject(aborted());
    }
    return new Promise((resolve, reject) => {
      const finish = (reply: FetchReply): void => {
        cleanup();
        resolve(reply);
      };
      const abort = (): void => {
        const index = this.pending.indexOf(finish);
        if (index >= 0) this.pending.splice(index, 1);
        cleanup();
        this.cancelled += 1;
        reject(aborted());
      };
      const cleanup = (): void => signal?.removeEventListener("abort", abort);
      signal?.addEventListener("abort", abort, { once: true });
      this.pending.push(finish);
    });
  }

  finish(reply: FetchReply): void {
    const finish = this.pending.shift();
    if (finish === undefined) {
      throw new Error("请求尚未派发");
    }
    finish(reply);
  }

  pendingCount(): number {
    return this.pending.length;
  }
}

function aborted(): Error {
  const error = new Error("aborted");
  error.name = "AbortError";
  return error;
}

/** 独立控制会话端点，记录外壳读取和登录认证头。 */
export interface Transport {
  readonly get: Stub;
  readonly post: Stub;
  readonly activity: Stub;
  readonly logout: Stub;
  pendingCountsCalls: number;
  readonly loginAuthorizations: string[];
  rejectLogoutRequest: boolean;
  rejectActivityRequest: boolean;
}

/** 会话请求手动完成，常规外壳读取立即返回。 */
export function transport(): Transport {
  return {
    get: new Stub(),
    post: new Stub(),
    activity: new Stub(),
    logout: new Stub(),
    pendingCountsCalls: 0,
    loginAuthorizations: [],
    rejectLogoutRequest: false,
    rejectActivityRequest: false,
  };
}

/** 构造保留原始状态、正文和可选响应头的回执。 */
export function response(
  status: number,
  body: string,
  headers?: Record<string, string>,
): FetchReply {
  return headers === undefined
    ? { kind: "response", status, body }
    : { kind: "response", status, body, headers };
}

/** 构造固定服务端时刻和截止的会话回执，允许显式更换 Cookie 代际。 */
export function sessionBody(csrfToken = "fixture-cookie", actor = "fixture-operator"): FetchReply {
  return response(
    200,
    JSON.stringify({
      code: "ok",
      message: "会话有效",
      data: {
        authenticated: true,
        actor,
        csrf_token: csrfToken,
        expires_at: "2026-11-18T00:15:00Z",
        absolute_expires_at: "2026-11-18T08:00:00Z",
        server_time: "2026-11-18T00:00:00Z",
      },
    }),
  );
}

function pendingCountsResponse(): Response {
  return new Response(
    JSON.stringify({
      code: "ok",
      message: "",
      data: {
        delivery_manual_reconcile: 0,
        recovery_confirmation: 0,
        active_alerts: 0,
        settlement_returns: 0,
      },
    }),
  );
}

/** 构造指定 HTTP 状态与稳定错误码的回执。 */
export function failure(
  status: number,
  code: string,
  headers?: Record<string, string>,
): FetchReply {
  return response(
    status,
    JSON.stringify({ code, message: "失败", data: null, request_id: null }),
    headers,
  );
}

/** 当前浏览器会话未认证的固定回执。 */
export function unauthorized(): FetchReply {
  return failure(401, "unauthenticated");
}

/** 替换 fetch/Request 以支持相对 Admin 路径；返回还原函数。 */
export function installFetch(stubs: Transport): () => void {
  const restoreFetch = preserveProperty(globalThis, "fetch");
  const restoreRequest = preserveProperty(globalThis, "Request");
  const originalRequest = globalThis.Request;
  class BrowserRequest extends originalRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      if (
        (stubs.rejectLogoutRequest && init?.method === "DELETE") ||
        (stubs.rejectActivityRequest &&
          init?.method === "POST" &&
          String(input).endsWith("/session/activity"))
      ) {
        throw new TypeError("请求构造失败替身");
      }
      super(
        typeof input === "string" && input.startsWith("/")
          ? new URL(input, "https://fixture.invalid/admin/")
          : input,
        init,
      );
    }
  }
  const restore = (): void => {
    try {
      restoreFetch();
    } finally {
      restoreRequest();
    }
  };
  try {
    Object.defineProperty(globalThis, "Request", {
      configurable: true,
      writable: true,
      value: BrowserRequest,
    });
    Object.defineProperty(globalThis, "fetch", {
      configurable: true,
      writable: true,
      value: async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
        const url = new URL(
          input instanceof Request ? input.url : String(input),
          "https://fixture.invalid",
        );
        const path = `${url.pathname}${url.search}`;
        const method = (init?.method ?? "GET").toUpperCase();
        const session = "/api/v1/admin/session";
        const sessionWithTime = `${session}?include=server_time`;
        let stub: Stub;
        switch (`${method} ${path}`) {
          case `GET ${sessionWithTime}`:
            stub = stubs.get;
            break;
          case `POST ${sessionWithTime}`:
            stubs.loginAuthorizations.push(new Headers(init?.headers).get("authorization") ?? "");
            stub = stubs.post;
            break;
          case `DELETE ${session}`:
            stub = stubs.logout;
            break;
          case `POST ${session}/activity`:
            stub = stubs.activity;
            break;
          case "GET /api/v1/admin/pending-counts?include=settlement_returns":
            stubs.pendingCountsCalls += 1;
            return pendingCountsResponse();
          case "GET /health/ready":
            return Response.json({ code: "ok", message: "", data: { ready: true } });
          default:
            throw new Error(`意外的认证测试请求 ${method} ${path}`);
        }
        const reply = await stub.request(init?.signal ?? undefined);
        if (reply.kind === "network") {
          throw new TypeError("network");
        }
        return new Response(reply.body, { status: reply.status, headers: reply.headers ?? {} });
      },
    });
  } catch (error) {
    restore();
    throw error;
  }
  return restore;
}
