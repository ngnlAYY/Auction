// Rust `delivery/tests.rs` 的详情 URL 与解除告警回执路径回归。

import { expect, spyOn, test } from "bun:test";
import {
  AdminDeliveryAcknowledgeResponseSchema,
  AdminDeliverySentProofPreviewResponseSchema,
} from "@auction/contracts/dto/admin/delivery";
import { decode } from "@auction/contracts/wire";
import { installFakeWindow } from "@auction/web-shared/router/test-window";

import { type AdminSession, type AdminSessionHost, registerAdminSessionHost } from "../session";
import { strictDeliveryEnvelopeSchema } from "./decode";
import {
  DELIVERY_ID,
  parseFixture,
  readAdminFixture,
  requireDecoded,
  requireElement,
  withoutField,
} from "./delivery-fixture.test";
import { acknowledgeDelivery, fetchDeliveryDetail, prepareAcknowledgeDelivery } from "./index";
import { decodePreview } from "./responses";

const ACKNOWLEDGE_ID = "00000000-0000-4000-8000-000000000023";
const COMMAND_ID = "00000000-0000-4000-8000-000000000024";
const REASON = "handled outside the outbox";
const SESSION_URL = "/api/v1/admin/session?include=server_time";
const SESSION: AdminSession = Object.freeze({
  actor: "operator",
  csrfToken: "csrf-token",
  csrf_token: "csrf-token",
  expiresAt: 0n,
  absoluteExpiresAt: 0n,
  serverTime: 0n,
  receivedAtMillis: 0,
});

const SESSION_BODY = JSON.stringify({
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

interface RecordedRequest {
  readonly method: string;
  readonly url: string;
  readonly body: string | null;
}

interface Reply {
  readonly body: string;
  readonly status: number;
}

async function withDeliveryTransport<T>(
  replies: readonly Reply[],
  run: (host: AdminSessionHost, requests: readonly RecordedRequest[]) => Promise<T>,
): Promise<T> {
  const requests: RecordedRequest[] = [];
  const originalRequest = globalThis.Request;
  // Bun 没有文档基址，但 Admin 的写传输仍要先构造相对路径的 Request。
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
  try {
    const fetchSpy = spyOn(globalThis, "fetch");
    try {
      fetchSpy.mockImplementation(
        Object.assign(
          async (input: URL | RequestInfo, init?: RequestInit) => {
            const url = input instanceof Request ? input.url : String(input);
            requests.push({
              method: init?.method ?? "GET",
              url,
              body: typeof init?.body === "string" ? init.body : null,
            });
            const reply = requireElement(replies, requests.length - 1, "内存 HTTP 回执");
            return new Response(reply.body, { status: reply.status });
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
        return await run(registration.host, requests);
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

async function postAcknowledgement(body: string, deliveryId: string, fencingVersion: number) {
  return withDeliveryTransport(
    [
      { body: SESSION_BODY, status: 200 },
      { body, status: 200 },
    ],
    async (host, requests) => {
      const prepared = requireDecoded(
        prepareAcknowledgeDelivery(deliveryId, fencingVersion, REASON)(COMMAND_ID),
        "解除告警命令准备",
      );
      const result = await acknowledgeDelivery(host, prepared, new AbortController().signal);
      expect(requests.map(({ method, url }) => [method, url])).toEqual([
        ["GET", SESSION_URL],
        ["POST", `/api/v1/admin/deliveries/${deliveryId}/acknowledge`],
      ]);
      const post = requireElement(requests, 1, "解除告警请求");
      expect(post.body === null ? null : parseFixture(post.body)).toEqual({
        command_id: COMMAND_ID,
        expected_fencing_version: fencingVersion,
        reason: REASON,
      });
      return result;
    },
  );
}

test("detail_url_requests_sent_proof_and_disposition_actions", async () => {
  const body = readAdminFixture("error-delivery-projection-excluded");
  await withDeliveryTransport([{ body, status: 404 }], async (host, requests) => {
    const result = await fetchDeliveryDetail(host, DELIVERY_ID, new AbortController().signal);
    expect(result).toEqual({ ok: false, error: "delivery_projection_excluded" });
    expect(requests).toEqual([
      {
        method: "GET",
        url: `/api/v1/admin/deliveries/${DELIVERY_ID}?include=sent_proof`,
        body: null,
      },
    ]);
  });
});

test("default_sent_preview_and_acknowledgement_contract_fixtures", async () => {
  const preview = parseFixture(readAdminFixture("delivery-sent-proof-preview-default"));
  const schema = strictDeliveryEnvelopeSchema(AdminDeliverySentProofPreviewResponseSchema);
  const payload = requireDecoded(decode(schema, preview), "缺省 Sent 预览信封");
  expect(payload.data.delivery_id).toBe(DELIVERY_ID);
  expect(payload.data.prior_unbound_attempts).toBeNull();
  expect(payload.data.sent_before_current_attempt).toBeNull();
  expect(decode(schema, withoutField(preview, "data", "eligible")).ok).toBe(false);
  expect(decodePreview(payload, DELIVERY_ID).ok).toBe(false);

  const bootstrap = await import("@auction/web-shared/ui/bootstrap");
  const window = installFakeWindow("/deliveries");
  try {
    const report = spyOn(bootstrap, "reportBrowserError");
    try {
      // 在存在窗口的环境中观察解码上报，保留对业务请求的精确计数。
      report.mockImplementation(() => undefined);
      const body = readAdminFixture("delivery-acknowledge-command");
      const receipt = requireDecoded(
        await postAcknowledgement(body, ACKNOWLEDGE_ID, 2),
        "解除告警回执",
      );
      expect(receipt.delivery_id).toBe(ACKNOWLEDGE_ID);
      expect(receipt.fencing_version).toBe(2);
      expect(receipt.kind).toBe("operator_acknowledged");
      expect(report).toHaveBeenCalledTimes(0);

      const missingKind = withoutField(parseFixture(body), "data", "kind");
      const receiptSchema = strictDeliveryEnvelopeSchema(AdminDeliveryAcknowledgeResponseSchema);
      expect(decode(receiptSchema, missingKind).ok).toBe(false);
      for (const [response, deliveryId, fencingVersion] of [
        [JSON.stringify(missingKind), ACKNOWLEDGE_ID, 2],
        [body, DELIVERY_ID, 2],
        [body, ACKNOWLEDGE_ID, 3],
      ] as const) {
        expect(await postAcknowledgement(response, deliveryId, fencingVersion)).toEqual<unknown>({
          ok: false,
          error: { kind: "outcome-unknown", responseStatus: null, code: "invalid_response" },
        });
        expect(report.mock.calls).toEqual([["dto_decode"]]);
        report.mockClear();
      }
    } finally {
      report.mockRestore();
    }
  } finally {
    window.restore();
  }
});
