// 投递处置命令的冻结正文、重试复用与端点正文形状。

import { expect, spyOn, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";

import { setAt } from "../api-request-fixture";
import { type AdminSession, type AdminSessionHost, registerAdminSessionHost } from "../session";
import {
  jsonObject,
  parseFixture,
  DELIVERY_ID as RUST_DELIVERY_ID,
  readAdminFixture,
  requireDecoded,
  requireElement,
} from "./delivery-fixture.test";
import {
  type DeliveryAction,
  type DeliveryCommandSnapshot,
  deliveryListUrl,
  deliverySearchFiltersDefault,
  executeDelivery,
  prepareDeliveryCommandOnce,
} from "./queue";

const DELIVERY_ID = "00000000-0000-4000-8000-000000000001";
const COMMAND_ID = "00000000-0000-4000-8000-000000000002";
const OTHER_COMMAND_ID = "00000000-0000-4000-8000-000000000003";

const SESSION: AdminSession = Object.freeze({
  actor: "operator",
  csrfToken: "csrf-token",
  csrf_token: "csrf-token",
  expiresAt: 0n,
  absoluteExpiresAt: 0n,
  serverTime: 0n,
  receivedAtMillis: 0,
});

interface RecordedRequest {
  readonly method: string;
  readonly url: string;
  readonly body: string | null;
}

interface MockAdminFetch {
  readonly calls: readonly string[];
  readonly requests: readonly RecordedRequest[];
}

/** 内存 fetch 替身：会话复核成功，写请求记录正文并返回用例指定的回执。 */
async function withAdminFetch(
  run: (host: AdminSessionHost, mock: MockAdminFetch) => Promise<void>,
  writeReply = () =>
    Response.json(
      { code: "stale_delivery_state", message: "stale", data: null, request_id: null },
      { status: 409 },
    ),
): Promise<void> {
  const calls: string[] = [];
  const requests: RecordedRequest[] = [];
  const originalRequest = globalThis.Request;
  try {
    // Bun 测试没有文档源，构造相对路径的 Request 时补上固定源。
    globalThis.Request = class extends originalRequest {
      constructor(input: RequestInfo | URL, init?: RequestInit) {
        super(
          typeof input === "string" && input.startsWith("/")
            ? new URL(input, "http://localhost")
            : input,
          init,
        );
      }
    };
    const fetchSpy = spyOn(globalThis, "fetch");
    try {
      fetchSpy.mockImplementation(
        Object.assign(
          async (input: URL | RequestInfo, init?: RequestInit) => {
            const url = String(input);
            const method = init?.method ?? "GET";
            calls.push(`${method} ${url}`);
            if (method === "GET" && url === "/api/v1/admin/session?include=server_time") {
              return Response.json({
                code: "ok",
                message: "",
                data: {
                  authenticated: true,
                  actor: "operator",
                  csrf_token: "csrf-token",
                  expires_at: "2099-01-01T00:00:00Z",
                  absolute_expires_at: "2099-01-02T00:00:00Z",
                  server_time: "2099-01-01T00:00:00Z",
                },
              });
            }
            requests.push({
              method,
              url,
              body: typeof init?.body === "string" ? init.body : null,
            });
            return writeReply();
          },
          { preconnect: () => {} },
        ),
      );
      const registration = registerAdminSessionHost({
        check: () => {},
        writesAllowed: () => true,
        confirmed: () => SESSION,
      });
      try {
        await run(registration.host, { calls, requests });
      } finally {
        registration.release();
      }
    } finally {
      fetchSpy.mockRestore();
    }
  } finally {
    globalThis.Request = originalRequest;
  }
}

function parseCommandBody(body: string | null) {
  if (body === null) throw new Error("缺少投递命令正文");
  const decoded = requireDecoded(parseJson(body), "投递命令正文解析");
  return jsonObject(decoded, "投递命令正文");
}

function deliverySnapshot(
  overrides: Partial<DeliveryCommandSnapshot> = {},
): DeliveryCommandSnapshot {
  return {
    deliveryId: DELIVERY_ID,
    action: "reconcile_retry",
    expectedFencingVersion: 7,
    externalReference: null,
    failureCode: null,
    ...overrides,
  };
}

test("同一个准备函数复制并冻结提交输入，同一命令 ID 复用同一份正文", () => {
  const input: {
    deliveryId: string;
    action: DeliveryAction;
    expectedFencingVersion: number;
    externalReference: string | null;
    failureCode: string | null;
  } = {
    deliveryId: DELIVERY_ID,
    action: "reconcile_retry",
    expectedFencingVersion: 7,
    externalReference: "original",
    failureCode: "verified_not_sent",
  };
  const preparation = prepareDeliveryCommandOnce(input);
  const first = preparation(COMMAND_ID);
  expect(first.ok).toBe(true);
  if (!first.ok) {
    return;
  }

  input.externalReference = "changed";
  input.failureCode = "recipient_invalid";
  input.expectedFencingVersion = 9;
  const second = preparation(COMMAND_ID);
  expect(second).toEqual(first);
  if (!second.ok) {
    return;
  }

  expect(Object.isFrozen(preparation.snapshot)).toBe(true);
  expect(preparation.snapshot).toEqual({
    deliveryId: DELIVERY_ID,
    action: "reconcile_retry",
    expectedFencingVersion: 7,
    externalReference: "original",
    failureCode: "verified_not_sent",
  });
  expect(second.value).toBe(first.value);
  expect(parseCommandBody(first.value.body)).toEqual({
    command_id: COMMAND_ID,
    expected_fencing_version: 7,
    action: "retry",
    external_reference: "original",
    failure_code: "verified_not_sent",
  });
});

test("换一个 command_id 被拒绝且不发请求", () =>
  withAdminFetch(async (host, { calls }) => {
    const preparation = prepareDeliveryCommandOnce(deliverySnapshot());
    expect(preparation(COMMAND_ID).ok).toBe(true);
    const result = await executeDelivery(
      host,
      preparation,
      OTHER_COMMAND_ID,
      new AbortController().signal,
    );
    expect(result).toEqual<unknown>(err({ kind: "not-sent", code: "invalid_request" }));
    expect(calls).toEqual([]);
  }));

test("/retry 省略 action，/reconcile 带 action: retry，两者省略空可选字段", () =>
  withAdminFetch(async (host, { requests }) => {
    const signal = new AbortController().signal;
    const retry = prepareDeliveryCommandOnce(deliverySnapshot({ action: "retry" }));
    const retryResult = await executeDelivery(host, retry, COMMAND_ID, signal);
    expect(retryResult.ok).toBe(false);

    const reconcile = prepareDeliveryCommandOnce(deliverySnapshot());
    const reconcileResult = await executeDelivery(host, reconcile, OTHER_COMMAND_ID, signal);
    expect(reconcileResult.ok).toBe(false);

    expect(requests.map((request) => request.url)).toEqual([
      `/api/v1/admin/deliveries/${DELIVERY_ID}/retry`,
      `/api/v1/admin/deliveries/${DELIVERY_ID}/reconcile`,
    ]);
    const retryBody = parseCommandBody(requireElement(requests, 0, "重试请求").body);
    expect(retryBody).toEqual({
      command_id: COMMAND_ID,
      expected_fencing_version: 7,
    });
    expect("action" in retryBody).toBe(false);
    expect("external_reference" in retryBody).toBe(false);
    expect("failure_code" in retryBody).toBe(false);

    const reconcileBody = parseCommandBody(requireElement(requests, 1, "对账请求").body);
    expect(reconcileBody).toEqual({
      command_id: OTHER_COMMAND_ID,
      expected_fencing_version: 7,
      action: "retry",
    });
    expect("external_reference" in reconcileBody).toBe(false);
    expect("failure_code" in reconcileBody).toBe(false);
  }));

test("两次独立提交各自持有正文，模块没有跨提交的状态", () => {
  const first = prepareDeliveryCommandOnce(deliverySnapshot({ externalReference: "first" }))(
    COMMAND_ID,
  );
  const second = prepareDeliveryCommandOnce(deliverySnapshot({ externalReference: "second" }))(
    COMMAND_ID,
  );
  expect(first.ok).toBe(true);
  expect(second.ok).toBe(true);
  if (!first.ok || !second.ok) {
    return;
  }

  expect(first.value).not.toBe(second.value);
  expect(parseCommandBody(first.value.body)).toEqual({
    command_id: COMMAND_ID,
    expected_fencing_version: 7,
    action: "retry",
    external_reference: "first",
  });
  expect(parseCommandBody(second.value.body)).toEqual({
    command_id: COMMAND_ID,
    expected_fencing_version: 7,
    action: "retry",
    external_reference: "second",
  });
});

test("list_url_requests_the_settlement_returns_projection", () => {
  expect(deliveryListUrl("", deliverySearchFiltersDefault(), 1, 50)).toBe(
    "/api/v1/admin/deliveries?page=1&page_size=50&include=settlement_returns",
  );
  const filters = {
    ...deliverySearchFiltersDefault(),
    world: "isekai",
    reference: RUST_DELIVERY_ID,
  };
  expect(deliveryListUrl("manual_reconcile", filters, 0, 20)).toBe(
    `/api/v1/admin/deliveries?page=1&page_size=20&status=manual_reconcile&reference=${RUST_DELIVERY_ID}&world=isekai&include=settlement_returns`,
  );
});

test("queue_reconcile_checks_target_and_new_fence", async () => {
  const deliveryId = "00000000-0000-4000-8000-000000000031";
  for (const defect of [null, "target", "fence", "body"]) {
    const wire = parseFixture(readAdminFixture("delivery-release-command"));
    if (defect === "target") setAt(wire, "/data/delivery/id", RUST_DELIVERY_ID);
    if (defect === "fence") setAt(wire, "/data/delivery/fencing_version", 4);
    await withAdminFetch(
      async (host, { requests }) => {
        const prepared = prepareDeliveryCommandOnce(
          deliverySnapshot({
            deliveryId,
            expectedFencingVersion: 4,
          }),
        );
        const result = await executeDelivery(
          host,
          prepared,
          COMMAND_ID,
          new AbortController().signal,
        );
        if (defect === null) {
          expect(requireDecoded(result, "有效重排回执")).toMatchObject({
            id: deliveryId,
            status: "pending",
            fencing_version: 5,
          });
        } else {
          expect(result).toEqual(
            err({
              kind: "outcome-unknown",
              responseStatus: null,
              code: "invalid_response",
            }),
          );
        }
        expect(requests.map(({ method, url }) => [method, url])).toEqual([
          ["POST", `/api/v1/admin/deliveries/${deliveryId}/reconcile`],
        ]);
        expect(parseCommandBody(requireElement(requests, 0, "重排请求").body)).toEqual({
          command_id: COMMAND_ID,
          action: "retry",
          expected_fencing_version: 4,
        });
      },
      () => (defect === "body" ? new Response("{") : Response.json(wire)),
    );
  }
});
