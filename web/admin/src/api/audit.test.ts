// 审计读取区分显式 null、缺键和分页模式，断言严格镜像在业务校验前拒绝畸形回执。

import { expect, spyOn, test } from "bun:test";
import { AdminAuditResponseSchema } from "@auction/contracts/dto/admin/audit";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { fixtureText, requestFixture, success } from "./auction-contract-fixture.test";
import { adminAuditFilterDefault, fetchAdminAudit } from "./audit";

function record() {
  return {
    id: "00000000-0000-4000-8000-000000000001",
    occurred_at: "2026-11-14T00:00:00Z",
    actor_type: "admin",
    actor_id: "operator",
    action: "auction.create",
    entity_type: "auction",
    entity_id: "00000000-0000-4000-8000-000000000002",
    result: "success",
    request_id: null,
    source: "admin",
    metadata: {},
  };
}

function page<T extends object>(records: readonly T[]) {
  return {
    has_more: false,
    next_cursor: null,
    next_cursor_token: "",
    page: 1,
    page_size: 20,
    records,
    total: 1,
  };
}

function numberedRead(body: string, size: number) {
  return requestFixture(
    [{ body }],
    [["GET", `/api/v1/admin/audit?page=1&page_size=${size}`]],
    (host, signal) => fetchAdminAudit(host, 1, adminAuditFilterDefault(), size, signal),
  );
}

function envelope(data: object): string {
  return JSON.stringify({ code: "ok", message: "", data });
}

async function expectSchemaRejection(body: string, size: number): Promise<void> {
  // 跟踪公开读取入口实际使用的私有严格镜像，记录生产解码器的原始返回值。
  const wire = await import("@auction/contracts/wire");
  const originalDecode = decode;
  const decoded: { readonly ok: boolean }[] = [];
  const decoder = spyOn(wire, "decode");
  try {
    decoder.mockImplementation((schema, input) => {
      const result = originalDecode(schema, input);
      decoded.push(result);
      return result;
    });
    expect(await numberedRead(body, size)).toEqual({ ok: false, error: "invalid_response" });
    expect(decoder).toHaveBeenCalledTimes(1);
    expect(decoded).toEqual([{ ok: false }]);
  } finally {
    decoder.mockRestore();
  }
}

test("explicit_null_cursor_and_request_id_decode_as_none", async () => {
  const decoded = success(await numberedRead(envelope(page([record()])), 20));
  expect(decoded.next_cursor).toBeNull();
  expect(decoded.records[0]?.request_id).toBeNull();
});

test("missing_nullable_audit_fields_fail_closed", async () => {
  const auditRecord = record();
  const complete = page([auditRecord]);
  expect(success(await numberedRead(envelope(complete), 20))).toEqual(complete);
  const { next_cursor: _cursor, ...missingCursor } = complete;
  await expectSchemaRejection(envelope(missingCursor), 20);
  const { request_id: _requestId, ...missingRequestId } = auditRecord;
  await expectSchemaRejection(envelope({ ...complete, records: [missingRequestId] }), 20);
});

test("numbered_audit_decodes_backend_fixture_and_keeps_cursor_mode_distinct", async () => {
  const numbered = success(await numberedRead(fixtureText("audit"), 50));
  expect(numbered.records[0]?.request_id).toBe("fixture-request");

  const cursor = fixtureText("audit-cursor");
  const dto = success(
    decode(apiResponseSchema(AdminAuditResponseSchema), success(parseJson(cursor))),
  );
  expect(dto.data.next_cursor).toEqual({
    before_id: expect.any(String),
    before_time: expect.any(String),
  });
  await expectSchemaRejection(cursor, 1);
});
