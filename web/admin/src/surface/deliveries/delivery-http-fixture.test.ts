// 投递与邮件测试的内存 HTTP 对端，保留真实读取、CSRF 复核与回执解码路径。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  AdminDeliveryDetailWithActionsResponseSchema,
  type AdminDeliverySentProofPreviewResponse,
} from "@auction/contracts/dto/admin/delivery";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import {
  installBrowserRequest,
  installFixtureFetch,
  parseJsonObject,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { cleanupScope } from "../../test-scope-fixture";
import { settle } from "../solid-dom-fixture";

const CONTRACT_FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);
const DELIVERY_FIXTURES = new URL(
  "../../../../../crates/auction-web-admin/src/api/delivery/",
  import.meta.url,
);

/** 真实投递详情样本的身份。 */
export const DELIVERY_ID = "0f8fad5b-d9cb-469f-a165-70867728950e";

/** 投递详情请求的固定地址。 */
export const DETAIL_GET =
  "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e?include=sent_proof";

/** 真实预览请求的地址，包含责任位投影。 */
export function previewPath(id: string): string {
  return `/api/v1/admin/deliveries/${id}/sent-proof/preview?include=responsibility_reasons`;
}

/** 读取 Rust 共用的投递详情样本，并按契约检查外壳。 */
export function detailFixtureWire(
  name: "sent-proof" | "blocked-registration" = "sent-proof",
): unknown {
  const url =
    name === "sent-proof"
      ? new URL("delivery-sent-proof-detail.json", CONTRACT_FIXTURES)
      : new URL("blocked_registration_reply.json", DELIVERY_FIXTURES);
  const source = parseJson(readFileSync(url, "utf8"));
  if (!source.ok) throw new Error("投递样本 JSON 无效");
  const raw = source.value;
  if (name === "sent-proof" && (!Array.isArray(raw) || raw.length !== 3)) {
    throw new Error("必须有三份人工 Sent 详情样本");
  }
  return Array.isArray(raw) ? raw[0] : raw;
}

/** 检查详情样本的契约并取出可供夹具使用的字段。 */
export function detailFixture(name: "sent-proof" | "blocked-registration" = "sent-proof") {
  const decoded = decode(
    apiResponseSchema(AdminDeliveryDetailWithActionsResponseSchema),
    detailFixtureWire(name),
  );
  if (!decoded.ok) throw new Error("投递详情样本解码失败");
  return decoded.value;
}

/** HTTP 请求记录保留正文，供用例检查真正发送的冻结参数。 */
export interface HttpRequest {
  readonly method: string;
  readonly path: string;
  readonly body: string;
}

/** 返回生产响应信封的内存回执。 */
export function jsonReply(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** 只为决定结果的预览读路径构造对应 Rust 替身的回执。 */
export function previewReply(preview: AdminDeliverySentProofPreviewResponse): Response {
  return jsonReply({ code: "ok", message: "预览准入", data: preview });
}

/** 预览的请求与详情相符，默认没有附加责任位。 */
export function previewData(
  id: string,
  fence: number,
  externalMailId: string,
): AdminDeliverySentProofPreviewResponse {
  return {
    eligible: true,
    blockers: [],
    delivery_id: id,
    expected_fencing_version: fence,
    external_reference: `mooglemail:main:send:${externalMailId}`,
    requires_collected_asset_acceptance: false,
    requires_attempt_history_acceptance: false,
    prior_unbound_attempts: false,
    sent_before_current_attempt: false,
    corrects_admin_failure: false,
  };
}

/** 确认或预览被当前 fence 拒绝。 */
export function staleDeliveryReply(): Response {
  return jsonReply(
    { code: "stale_delivery_state", message: "投递状态已变化", data: null, request_id: null },
    409,
  );
}

/** Rust 测试所用的暂不可用读取结果。 */
export function unavailableReply(): Response {
  return jsonReply(
    { code: "storage_unavailable", message: "暂不可用", data: null, request_id: null },
    503,
  );
}

async function abortableReply(
  signal: AbortSignal,
  reply: () => Response | Promise<Response>,
): Promise<Response> {
  signal.throwIfAborted();
  const aborted = Promise.withResolvers<never>();
  const cancel = () => aborted.reject(signal.reason);
  signal.addEventListener("abort", cancel, { once: true });
  try {
    return await Promise.race([aborted.promise, Promise.resolve().then(reply)]);
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}

/** 在用例内拦截 fetch；CSRF 复核照常工作，业务请求由用例按路径应答。 */
export async function withHttp(
  respond: (request: HttpRequest) => Response | Promise<Response>,
  run: (requests: readonly HttpRequest[]) => Promise<void>,
): Promise<void> {
  const scope = cleanupScope();
  const recorded: HttpRequest[] = [];
  const lifetime = new AbortController();
  const pending = new Set<Promise<Response>>();
  try {
    scope.defer(installBrowserRequest());
    scope.defer(
      installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        const url = new URL(request.url, "https://auction.test");
        if (url.origin !== "https://auction.test") {
          throw new Error(`请求逃离内存对端：${url.origin}`);
        }
        const entry: HttpRequest = {
          method: request.method,
          path: url.pathname + url.search,
          body: request.body,
        };
        recorded.push(entry);
        const requestSignal = init?.signal ?? (input instanceof Request ? input.signal : null);
        const signal =
          requestSignal === null
            ? lifetime.signal
            : AbortSignal.any([requestSignal, lifetime.signal]);
        const response = abortableReply(signal, () =>
          entry.method === "GET" && entry.path === "/api/v1/admin/session?include=server_time"
            ? jsonReply(TEST_SESSION_BODY)
            : respond(entry),
        );
        pending.add(response);
        try {
          return await response;
        } finally {
          pending.delete(response);
        }
      }),
    );
    await run(recorded);
  } finally {
    try {
      lifetime.abort();
      await Promise.allSettled(pending);
      // fetch 终止后让真实传输层消费拒绝并取消请求计时器。
      await settle();
    } finally {
      scope.dispose();
    }
  }
}

/** 提取请求正文的 JSON 对象，供用例核对请求参数。 */
export function bodyOf(request: HttpRequest): Record<string, unknown> {
  return { ...parseJsonObject(request.body, "请求正文") };
}

/** 保留业务请求的方法、路径和顺序，排除固定的会话复核。 */
export function businessRequests(requests: readonly HttpRequest[]): string[] {
  return requests
    .filter((request) => request.path !== "/api/v1/admin/session?include=server_time")
    .map((request) => `${request.method} ${request.path}`);
}

test("投递 HTTP 夹具保留正文和请求顺序，过滤会话复核视图", async () => {
  const originalFetch = globalThis.fetch;
  await withHttp(
    () => jsonReply({ code: "ok" }, 202),
    async (requests) => {
      const session = await fetch("/api/v1/admin/session?include=server_time");
      const path = "/fixture?kind=delivery";
      const body = '{ "version": 1 }';
      const response = await fetch(path, { method: "POST", body });
      expect(session.status).toBe(200);
      expect(response.status).toBe(202);
      expect(requests).toEqual([
        { method: "GET", path: "/api/v1/admin/session?include=server_time", body: "" },
        { method: "POST", path, body },
      ]);
      expect(businessRequests(requests)).toEqual([`POST ${path}`]);
    },
  );
  expect(globalThis.fetch).toBe(originalFetch);
});
