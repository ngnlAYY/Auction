// 用独立的协议样本表核对错误状态、拒绝范围和两种未决历史。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { integer, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import * as v from "valibot";

import { writeFailureFromReply } from "./api-request-fixture";
import {
  type AdminWriteEndpoint,
  adminWriteFailureDisplayCode,
  classifyAdminWriteFailure,
  observedResponseStatus,
} from "./failure";
import { runHistoryCase } from "./failure-history-fixture.test";
import { writeRejectionCatalog } from "./rejection";

// D2 的普通码表中，14 个账本重放码另用后端契约样本测试。501 来自 F10 的裁定。
const THIS_ATTEMPT_CASES = [
  [
    400,
    [
      "invalid_request_id",
      "invalid_query",
      "invalid_json",
      "invalid_replay",
      "invalid_command_id",
      "invalid_auction_id",
      "invalid_auction_command",
      "invalid_auction_schedule",
      "invalid_auction_intake",
      "unsupported_equipment_source",
      "invalid_lot_id",
      "invalid_lot_action",
      "invalid_lot_command",
      "invalid_recovery_command",
      "invalid_world",
      "invalid_world_command",
      "invalid_delivery_id",
      "invalid_user_id",
      "invalid_user_command",
      "invalid_rename_id",
      "invalid_rename_command",
      "invalid_delivery_command",
      "invalid_alert_command",
      "invalid_credential_action",
      "invalid_credential_kind",
      "invalid_credential_label",
      "invalid_credential_player_name",
      "invalid_credential_request",
      "invalid_credential_secret",
      "invalid_credential_status",
      "invalid_credential_world",
      "invalid_mail_request",
      "invalid_runtime_config",
      "invalid_locale",
      "invalid_rule_document",
      "invalid_settlement_return_command",
      "invalid_settlement_return_id",
    ],
  ],
  [401, ["unauthenticated"]],
  [403, ["origin_forbidden", "csrf_invalid", "manual_early_settlement_disabled"]],
  [
    404,
    [
      "not_found",
      "mail_template_not_found",
      "world_not_found",
      "delivery_not_found",
      "settlement_return_not_found",
    ],
  ],
  [405, ["method_not_allowed"]],
  [
    409,
    [
      "command_conflict",
      "credential_conflict",
      "stale_credential_state",
      "rename_name_taken",
      "user_conflict",
      "stale_state_version",
      "stale_world_state",
      "world_has_active_auction",
      "auction_intake_unavailable",
      "seller_blacklisted",
      "equipment_conflict",
      "stale_recovery_state",
      "stale_delivery_state",
      "invalid_auction_state",
      "invalid_lot_state",
      "delivery_sent_sighted",
      "delivery_recipient_renamed",
      "delivery_command_snapshot_unreadable",
      "settlement_obligation_ineligible",
      "sent_proof_rejected",
      "settlement_return_action_conflict",
      "settlement_return_resend_limit",
      "settlement_return_obligation_ineligible",
      "settlement_return_send_unresolved",
      "settlement_return_observation_required",
      "recovery_send_unattributed",
      "recovery_not_confirmable",
      "recovery_evidence_unresolved",
      "external_access_paused",
      "poll_rejected",
    ],
  ],
  [413, ["body_too_large"]],
  [415, ["unsupported_media_type"]],
  [
    422,
    [
      "invalid_mail_template",
      "invalid_runtime_switch",
      "not_acknowledgeable",
      "credential_login_rejected",
      "credential_login_challenge",
    ],
  ],
  [429, ["rate_limited", "credential_login_busy"]],
  [501, ["feature_not_supported"]],
  [503, ["credential_login_unavailable"]],
] as const;

const VERSION_CONFLICT_CASES = [
  ["general", false],
  ["mail-template", false],
  ["rename-revert", false],
  ["user-mutation", true],
  ["rules-publish", true],
  ["about-publish", true],
] as const satisfies ReadonlyArray<readonly [AdminWriteEndpoint, boolean]>;

function errorBody(code: string): string {
  return JSON.stringify({ code, message: "请求已拒绝", data: null, request_id: null });
}

const WRITE_REJECTIONS = writeRejectionCatalog();

const SHARED_RECEIPT_SAMPLES = strictStruct({
  description: v.string(),
  histories: v.tuple([v.literal("first_attempt"), v.literal("prior_unknown")]),
  groups: v.array(
    strictStruct({
      endpoints: v.pipe(v.array(v.picklist(WRITE_REJECTIONS.endpoints)), v.nonEmpty()),
      protocol_codes: v.pipe(v.array(v.string()), v.nonEmpty()),
      responses: v.pipe(
        v.array(
          strictStruct({
            http_status: integer,
            ruling: v.picklist(["unknown", "this_attempt", "replay_checked"]),
            display_code: v.nullable(v.string()),
          }),
        ),
        v.nonEmpty(),
      ),
    }),
  ),
});

function expandSharedReceipts(groups: v.InferOutput<typeof SHARED_RECEIPT_SAMPLES>["groups"]) {
  return groups.flatMap((group) =>
    group.endpoints.flatMap((endpoint) =>
      group.protocol_codes.flatMap((code) =>
        group.responses.map((response) => ({ endpoint, code, response })),
      ),
    ),
  );
}

function assertSampleCoverage(cases: ReturnType<typeof expandSharedReceipts>): void {
  const { endpoints, rules } = WRITE_REJECTIONS;
  const expectedCodes = new Set([...rules.map(({ code }) => code), "state_conflict"]);
  const sampleCodes = new Set(cases.map(({ code }) => code));
  const sampleEndpoints = new Set(cases.map(({ endpoint }) => endpoint));
  expect(
    [...expectedCodes].filter((code) => !sampleCodes.has(code)),
    "样本缺少协议码",
  ).toEqual([]);
  expect(
    [...sampleCodes].filter((code) => !expectedCodes.has(code)),
    "样本多出协议码",
  ).toEqual([]);
  expect([...sampleEndpoints].sort(), "样本须覆盖端点类型的全部标签").toEqual(
    [...endpoints].sort(),
  );

  for (const { code, status, scope } of rules) {
    const replies = cases.filter((sample) => sample.code === code).map(({ response }) => response);
    const hasReply = (httpStatus: number, ruling: (typeof replies)[number]["ruling"]) =>
      replies.some((reply) => reply.http_status === httpStatus && reply.ruling === ruling);
    if (scope === "this_attempt") {
      expect(hasReply(status, "this_attempt"), `样本缺少 ${code} HTTP ${status} 正例`).toBe(true);
      expect(
        replies.some((reply) => reply.http_status !== status && reply.ruling === "unknown"),
        `样本缺少 ${code} 的错误状态反例`,
      ).toBe(true);
    } else {
      expect(status, `重放拒绝 ${code} 必须匹配 HTTP 409`).toBe(409);
      expect(hasReply(409, "replay_checked"), `样本缺少 ${code} HTTP 409 正例`).toBe(true);
      for (const wrong of [400, 401, 403, 404, 422, 429, 500, 502, 503, 504]) {
        expect(hasReply(wrong, "unknown"), `样本缺少 ${code} HTTP ${wrong} 反例`).toBe(true);
      }
    }
  }
  for (const endpoint of endpoints) {
    expect(
      cases.some(
        (sample) =>
          sample.endpoint === endpoint &&
          sample.code === "state_conflict" &&
          sample.response.http_status === 409,
      ),
      `样本缺少 ${endpoint} HTTP 409 state_conflict`,
    ).toBe(true);
  }
}

test("共用写回执样本与未决命令宿主的裁定一致", async () => {
  const fixture = new URL(
    "../../../../crates/auction-contracts/tests/fixtures/admin/write-receipt-rejections.json",
    import.meta.url,
  );
  const parsed = parseJson(readFileSync(fixture, "utf8"));
  if (!parsed.ok) throw new Error("写回执样本须为合法 JSON");
  const { groups } = v.parse(SHARED_RECEIPT_SAMPLES, parsed.value);
  const cases = expandSharedReceipts(groups);
  assertSampleCoverage(cases);

  for (const { endpoint, code, response } of cases) {
    const status = response.http_status;
    const label = `${endpoint} HTTP ${status} ${code}`;
    const display = response.display_code ?? code;
    const failure = await writeFailureFromReply(status, errorBody(code), endpoint);
    expect(classifyAdminWriteFailure(failure), label).toBe(response.ruling);
    expect(adminWriteFailureDisplayCode(failure), label).toBe(display);
    await runHistoryCase(failure, { display, scope: response.ruling });
  }
});

test.each(
  THIS_ATTEMPT_CASES.flatMap(([status, codes]) => codes.map((code) => [code, status] as const)),
)("%s 只在 HTTP %i 裁定本次拒绝", async (code, expectedStatus) => {
  const body = errorBody(code);
  const rejection = await writeFailureFromReply(expectedStatus, body);
  expect(rejection.kind).toBe("verified-http");
  expect(observedResponseStatus(rejection)).toBe(expectedStatus);
  expect(adminWriteFailureDisplayCode(rejection)).toBe(code);
  expect(classifyAdminWriteFailure(rejection)).toBe("this_attempt");
  await runHistoryCase(rejection, { display: code, scope: "this_attempt" });

  for (const [status, display] of [
    [418, "http_418"],
    [500, "http_500"],
  ] as const) {
    const mismatch = await writeFailureFromReply(status, body);
    expect(observedResponseStatus(mismatch)).toBe(status);
    expect(adminWriteFailureDisplayCode(mismatch)).toBe(display);
    expect(classifyAdminWriteFailure(mismatch)).toBe("unknown");
    await runHistoryCase(mismatch, { display, scope: "unknown" });
  }
});

test.each(VERSION_CONFLICT_CASES)(
  "%s 的 state_conflict 按端点和状态裁定",
  async (endpoint, versioned) => {
    const body = errorBody("state_conflict");
    const failure = await writeFailureFromReply(409, body, endpoint);
    const scope = versioned ? "replay_checked" : "unknown";
    expect(observedResponseStatus(failure)).toBe(409);
    expect(classifyAdminWriteFailure(failure)).toBe(scope);
    await runHistoryCase(failure, { display: "state_conflict", scope });

    for (const status of [400, 503]) {
      const mismatch = await writeFailureFromReply(status, body, endpoint);
      const display = versioned ? `http_${status}` : "state_conflict";
      expect(observedResponseStatus(mismatch)).toBe(status);
      expect(classifyAdminWriteFailure(mismatch)).toBe("unknown");
      await runHistoryCase(mismatch, { display, scope: "unknown" });
    }
  },
);
