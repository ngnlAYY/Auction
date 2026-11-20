// Admin 写命令失败分类：拒绝证据、重试边界与 HTTP 状态精确匹配回归。

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { parseJson } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";

import {
  type AdminApiFailure,
  type AdminWriteEndpoint,
  adminApiFailureAtWriteEndpoint,
  adminApiFailureClient,
  adminApiFailureDefiniteRejection,
  adminApiFailureDisplayCode,
  adminApiFailureFromHttpBody,
  adminApiFailureNotSentCode,
  adminApiFailureOutcomeUnknown,
  adminApiFailureRejectionAfterReplayCheck,
  readResponseFailure,
} from "./failure";
import { runHistoryCase, runMailTemplateHistory } from "./failure-history-fixture.test";
import { jsonObject } from "./json-fixture";

const fixtures = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixture(name: string): string {
  return readFileSync(new URL(name, fixtures), "utf8");
}

function completeBody(code: string, message: string): string {
  return JSON.stringify({ code, message, data: null, request_id: null });
}

function fromHttp(status: number, code: string): AdminApiFailure {
  // TS 没有导出 Rust 的 `from_http` 构造器；这里经同一公开解码入口得到状态与规范化码。
  const body = completeBody(code, "请求已拒绝");
  return adminApiFailureFromHttpBody(status, body);
}

/** 已读入内存的响应；失败分类只消费状态与原始正文字节。 */
function transportResponse(status: number, body: string): TransportResponse {
  const bytes = new TextEncoder().encode(body);
  return {
    status,
    statusText: "",
    headers: new Headers(),
    body: bytes,
    text: () => body,
    json: () => err({ kind: "decode" }),
  };
}

/** 经真实回执解码与写端点归一化得到宿主最终收到的稳定码。 */
function failureAt(status: number, body: string, endpoint: AdminWriteEndpoint): AdminApiFailure {
  return adminApiFailureAtWriteEndpoint(
    readResponseFailure(transportResponse(status, body)),
    endpoint,
  );
}

/** 从可信 409 样本改写 `data`，复现服务端回执形状损坏时的分类。 */
function fixtureWithData(data: unknown): string {
  const parsed = parseJson(fixture("error-mail-template-conflict.json"));
  if (!parsed.ok) {
    throw new Error("错误信封不是合法 JSON");
  }
  const envelope = structuredClone(jsonObject(parsed.value, "错误信封"));
  envelope.data = data;
  return JSON.stringify(envelope);
}

describe("Admin API 失败分类（对应 Rust api/failure.rs）", () => {
  test("delivery_guards_reject_this_attempt_without_clearing_an_earlier_unknown", () => {
    for (const code of ["delivery_recipient_renamed", "delivery_command_snapshot_unreadable"]) {
      const body = completeBody(code, "投递动作已拒绝");
      const failure = adminApiFailureFromHttpBody(409, body);
      expect(adminApiFailureOutcomeUnknown(failure)).toBe(false);
      expect(adminApiFailureRejectionAfterReplayCheck(failure.code)).toBe(false);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureFromHttpBody(503, body))).toBe(true);
    }
  });

  test("credential_import_rejections_only_resolve_this_attempt", () => {
    const cases: ReadonlyArray<readonly [number, string]> = [
      [400, "invalid_credential_kind"],
      [400, "invalid_credential_world"],
      [400, "invalid_credential_request"],
      [400, "invalid_credential_label"],
      [400, "invalid_credential_player_name"],
      [400, "invalid_credential_secret"],
      [409, "credential_conflict"],
      [422, "credential_login_rejected"],
      [422, "credential_login_challenge"],
      [429, "credential_login_busy"],
      [503, "credential_login_unavailable"],
    ];
    for (const [status, code] of cases) {
      const body = completeBody(code, "凭据导入已拒绝");
      const failure = adminApiFailureFromHttpBody(status, body);
      expect(adminApiFailureOutcomeUnknown(failure), code).toBe(false);
      expect(adminApiFailureRejectionAfterReplayCheck(failure.code), code).toBe(false);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureFromHttpBody(502, body))).toBe(true);
    }
    const unavailable = adminApiFailureFromHttpBody(
      503,
      '{"code":"storage_unavailable","message":"凭据存储不可用","data":null,"request_id":null}',
    );
    expect(adminApiFailureOutcomeUnknown(unavailable)).toBe(true);
  });

  test("only_complete_error_envelopes_can_be_used_as_rejection_evidence", () => {
    for (const [status, code] of [
      [413, "body_too_large"],
      [409, "mail_template_conflict"],
    ] as const) {
      const complete: Record<string, unknown> = {
        code,
        message: "请求已拒绝",
        data: null,
        request_id: null,
      };
      const verified = adminApiFailureFromHttpBody(status, JSON.stringify(complete));
      expect(adminApiFailureOutcomeUnknown(verified)).toBe(false);
      expect(adminApiFailureRejectionAfterReplayCheck(verified.code)).toBe(status === 409);

      for (const key of ["code", "message", "data", "request_id"]) {
        const missing = { ...complete };
        expect(Object.hasOwn(missing, key), `缺少前字段存在 ${key}`).toBe(true);
        delete missing[key];
        const rejected = adminApiFailureFromHttpBody(status, JSON.stringify(missing));
        expect(adminApiFailureOutcomeUnknown(rejected), `缺少 ${key}`).toBe(true);
        expect(adminApiFailureDefiniteRejection(rejected.code), `缺少 ${key}`).toBe(false);
        expect(adminApiFailureRejectionAfterReplayCheck(rejected.code), `缺少 ${key}`).toBe(false);
      }

      for (const [key, invalid] of [
        ["code", 409],
        ["message", null],
        ["request_id", true],
        ["extra", null],
      ] as const) {
        const malformed = { ...complete, [key]: invalid };
        const rejected = adminApiFailureFromHttpBody(status, JSON.stringify(malformed));
        expect(adminApiFailureOutcomeUnknown(rejected), `字段不符 ${key}`).toBe(true);
        expect(adminApiFailureDefiniteRejection(rejected.code), `字段不符 ${key}`).toBe(false);
        expect(adminApiFailureRejectionAfterReplayCheck(rejected.code), `字段不符 ${key}`).toBe(
          false,
        );
      }

      const populated = {
        ...complete,
        request_id: "admin-receipt",
        data: { revision: 6 },
      };
      expect(
        adminApiFailureOutcomeUnknown(
          adminApiFailureFromHttpBody(status, JSON.stringify(populated)),
        ),
      ).toBe(false);

      const positional = JSON.stringify([code, "请求已拒绝", null, null]);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureFromHttpBody(status, positional))).toBe(
        true,
      );
    }
  });

  test("only_unknown_outcomes_are_retried_verbatim", () => {
    expect(adminApiFailureOutcomeUnknown(adminApiFailureClient("network_error"))).toBe(true);
    expect(adminApiFailureOutcomeUnknown({ status: 503, code: "outcome_unknown" })).toBe(true);
    expect(adminApiFailureOutcomeUnknown({ status: 502, code: "http_502" })).toBe(true);
    expect(adminApiFailureOutcomeUnknown({ status: 422, code: "unrecognized_rejection" })).toBe(
      true,
    );
    expect(adminApiFailureOutcomeUnknown(adminApiFailureClient("invalid_request"))).toBe(false);
    for (const [status, code] of [
      [422, "invalid_mail_template"],
      [409, "mail_template_conflict"],
    ] as const) {
      expect(adminApiFailureOutcomeUnknown({ status, code })).toBe(false);
    }
  });

  test("only_the_backend_413_envelope_unlocks_a_rejected_write", async () => {
    const tooLarge = fixture("mail-template-body-too-large.json");
    const rejected = adminApiFailureAtWriteEndpoint(
      adminApiFailureFromHttpBody(413, tooLarge),
      "mail-template",
    );
    expect(adminApiFailureOutcomeUnknown(rejected)).toBe(false);
    expect(adminApiFailureDisplayCode(rejected.code)).toBe("body_too_large");
    expect(adminApiFailureRejectionAfterReplayCheck(rejected.code)).toBe(false);
    expect(adminApiFailureOutcomeUnknown(fromHttp(413, "http_413"))).toBe(true);
    // 非契约或空正文不能提供拒绝证据，413 只保留 HTTP 兜底并维持未知结果。
    for (const body of ["", "not json", "[]", '{"code":413}']) {
      const fallback = adminApiFailureFromHttpBody(413, body);
      expect(fallback).toEqual({ status: 413, code: "http_413" });
      expect(adminApiFailureOutcomeUnknown(fallback)).toBe(true);
      expect(adminApiFailureDefiniteRejection(fallback.code)).toBe(false);
    }
    const unavailable = fromHttp(503, rejected.code);
    expect(adminApiFailureOutcomeUnknown(unavailable)).toBe(true);
    expect(adminApiFailureDefiniteRejection(unavailable.code)).toBe(false);
    await runMailTemplateHistory("413 后同一命令收到可信 409", [
      { target: "write", status: 413, body: tooLarge, display: "body_too_large", clears: false },
      {
        target: "write",
        status: 409,
        body: fixture("error-mail-template-conflict.json"),
        display: "mail_template_conflict",
        clears: true,
      },
    ]);
  });

  test("a_first_user_mutation_version_conflict_is_definite", () => {
    const receipt = adminApiFailureFromHttpBody(409, fixture("error-user-state-conflict.json"));
    const rejection = adminApiFailureAtWriteEndpoint(receipt, "user-mutation");
    expect(rejection.status).toBe(409);
    expect(adminApiFailureOutcomeUnknown(rejection)).toBe(false);
    expect(adminApiFailureRejectionAfterReplayCheck(rejection.code)).toBe(true);
    expect(adminApiFailureDefiniteRejection(rejection.code)).toBe(true);
    expect(adminApiFailureDisplayCode(rejection.code)).toBe("state_conflict");
    expect(adminApiFailureOutcomeUnknown(receipt)).toBe(true);
  });

  test("a_first_rules_or_about_publication_version_conflict_is_definite", () => {
    for (const endpoint of ["rules-publish", "about-publish"] as AdminWriteEndpoint[]) {
      const rejection = adminApiFailureAtWriteEndpoint(
        adminApiFailureFromHttpBody(409, fixture("error-document-state-conflict.json")),
        endpoint,
      );
      expect(rejection.status).toBe(409);
      expect(adminApiFailureOutcomeUnknown(rejection)).toBe(false);
      expect(adminApiFailureRejectionAfterReplayCheck(rejection.code)).toBe(true);
      expect(adminApiFailureDefiniteRejection(rejection.code)).toBe(true);
      for (const status of [400, 500]) {
        const mismatch = adminApiFailureAtWriteEndpoint(
          adminApiFailureFromHttpBody(status, fixture("error-document-state-conflict.json")),
          endpoint,
        );
        expect(adminApiFailureOutcomeUnknown(mismatch)).toBe(true);
        expect(adminApiFailureDefiniteRejection(mismatch.code)).toBe(false);
      }
    }
  });

  test("a_server_code_cannot_claim_the_endpoint_or_preflight_marker", () => {
    const tagged = adminApiFailureAtWriteEndpoint(fromHttp(409, "state_conflict"), "user-mutation");
    for (const code of [tagged.code, adminApiFailureNotSentCode("network_error")]) {
      const forged = fromHttp(409, code);
      expect(adminApiFailureOutcomeUnknown(forged)).toBe(true);
      expect(adminApiFailureDefiniteRejection(forged.code)).toBe(false);
    }
  });

  test("reversible_rejections_do_not_resolve_an_earlier_attempt", () => {
    for (const code of [
      "rename_name_taken",
      "invalid_lot_state",
      "stale_recovery_state",
      "stale_delivery_state",
      "stale_world_state",
      "not_acknowledgeable",
      "delivery_sent_sighted",
      "settlement_obligation_ineligible",
      "settlement_return_action_conflict",
      "settlement_return_resend_limit",
      "settlement_return_obligation_ineligible",
      "settlement_return_send_unresolved",
      "settlement_return_observation_required",
    ]) {
      expect(adminApiFailureDefiniteRejection(code), code).toBe(true);
      expect(adminApiFailureRejectionAfterReplayCheck(code), code).toBe(false);
    }
    for (const name of [
      "error-stale-delivery-state.json",
      "error-stale-delivery-disposition.json",
    ]) {
      const failure = adminApiFailureFromHttpBody(409, fixture(name));
      expect(failure.code).toBe("stale_delivery_state");
      expect(failure.status).toBe(409);
      expect(adminApiFailureOutcomeUnknown(failure)).toBe(false);
      expect(adminApiFailureRejectionAfterReplayCheck(failure.code)).toBe(false);
    }
    const conflict = adminApiFailureAtWriteEndpoint(
      adminApiFailureFromHttpBody(409, fixture("error-rename-name-taken.json")),
      "rename-revert",
    );
    expect(adminApiFailureOutcomeUnknown(conflict)).toBe(false);
    expect(adminApiFailureRejectionAfterReplayCheck(conflict.code)).toBe(false);
  });

  test("reclassified_rejections_require_the_exact_http_status", () => {
    const cases: ReadonlyArray<readonly [number, string]> = [
      [400, "invalid_json"],
      [400, "invalid_auction_intake"],
      [400, "unsupported_equipment_source"],
      [403, "manual_early_settlement_disabled"],
      [404, "not_found"],
      [404, "settlement_return_not_found"],
      [409, "stale_lot_state"],
      [409, "command_conflict"],
      [409, "credential_conflict"],
      [409, "stale_credential_state"],
      [409, "auction_intake_unavailable"],
      [409, "seller_blacklisted"],
      [409, "equipment_conflict"],
      [422, "not_acknowledgeable"],
      [422, "credential_login_rejected"],
      [422, "credential_login_challenge"],
      [429, "rate_limited"],
      [429, "credential_login_busy"],
    ];
    for (const [expected, code] of cases) {
      const rejection = fromHttp(expected, code);
      expect(adminApiFailureOutcomeUnknown(rejection), code).toBe(false);
      expect(adminApiFailureDefiniteRejection(rejection.code), code).toBe(true);
      expect(rejection.code, code).toBe(code);
      for (const status of [418, 500]) {
        const mismatch = fromHttp(status, code);
        expect(adminApiFailureOutcomeUnknown(mismatch), `${code} ${status}`).toBe(true);
        expect(adminApiFailureDefiniteRejection(mismatch.code), `${code} ${status}`).toBe(false);
      }
    }
  });

  test("only_verified_replay_checked_rejections_can_clear_unknown_history", async () => {
    const replayChecked: ReadonlyArray<readonly [string, string]> = [
      ["alert_stale", "error-alert-stale.json"],
      ["stale_auction_state", "error-stale-auction-state.json"],
      ["stale_lot_state", "error-stale-lot-state.json"],
      ["stale_user_state", "error-stale-user-state.json"],
      ["rename_not_revertable", "error-rename-not-revertable.json"],
      ["mail_template_conflict", "error-mail-template-conflict.json"],
      ["runtime_switch_conflict", "error-runtime-switch-conflict.json"],
      ["already_disposed", "error-already-disposed.json"],
      ["stale_settlement_return_state", "error-stale-settlement-return-state.json"],
      ["settlement_return_untrusted", "error-settlement-return-untrusted.json"],
      ["settlement_return_unbound", "error-settlement-return-unbound.json"],
      ["settlement_return_assets_mismatch", "error-settlement-return-assets-mismatch.json"],
      ["settlement_return_proof_required", "error-settlement-return-proof-required.json"],
      ["conflict", "error-realtime-conflict.json"],
    ];
    for (const [code, name] of replayChecked) {
      const body = fixture(name);
      const rejection = adminApiFailureFromHttpBody(409, body);
      expect(rejection.code).toBe(code);
      expect(rejection.status).toBe(409);
      expect(adminApiFailureOutcomeUnknown(rejection), code).toBe(false);
      expect(adminApiFailureRejectionAfterReplayCheck(rejection.code), code).toBe(true);
      const clientFailure = adminApiFailureClient(rejection.code);
      expect(clientFailure.status, code).toBeNull();
      expect(adminApiFailureRejectionAfterReplayCheck(clientFailure.code), code).toBe(true);
      for (const status of [400, 503]) {
        const mismatch = adminApiFailureFromHttpBody(status, body);
        expect(mismatch.status, `${code} ${status}`).toBe(status);
        expect(adminApiFailureOutcomeUnknown(mismatch), `${code} ${status}`).toBe(true);
        expect(adminApiFailureRejectionAfterReplayCheck(mismatch.code), `${code} ${status}`).toBe(
          false,
        );
      }
      // 会话 GET 与实际写请求使用同一份样本，只有后者的 409 是旧命令的重放证据。
      await runMailTemplateHistory(code, [
        { target: "session", status: 409, body, display: code, clears: false },
        { target: "write", status: 400, body, display: "http_400", clears: false },
        { target: "write", status: 503, body, display: "http_503", clears: false },
        { target: "write", status: 409, body, display: code, clears: true },
      ]);
    }

    const rejection = adminApiFailureFromHttpBody(409, fixture("error-alert-stale.json"));
    const notSent = adminApiFailureNotSentCode(rejection.code);
    expect(adminApiFailureDefiniteRejection(notSent)).toBe(true);
    expect(adminApiFailureDisplayCode(notSent)).toBe("alert_stale");
    expect(adminApiFailureRejectionAfterReplayCheck(notSent)).toBe(false);
    for (const code of ["invalid_alert_command", "stale_credential_state", "rate_limited"]) {
      expect(adminApiFailureDefiniteRejection(code)).toBe(true);
      expect(adminApiFailureRejectionAfterReplayCheck(code)).toBe(false);
    }
    for (const status of [400, 500]) {
      const mismatch = fromHttp(status, rejection.code);
      expect(adminApiFailureOutcomeUnknown(mismatch)).toBe(true);
      expect(adminApiFailureRejectionAfterReplayCheck(mismatch.code)).toBe(false);
    }
  });

  test("pending_command_history_keeps_unknown_until_a_replay_checked_rejection", async () => {
    const conflict = fixture("error-mail-template-conflict.json");
    const cases: ReadonlyArray<readonly [string, string, boolean]> = [
      ["无 HTTP 状态的 network_error", adminApiFailureClient("network_error").code, false],
      [
        "无 HTTP 状态的未发送标记",
        adminApiFailureClient(adminApiFailureNotSentCode("request_timeout")).code,
        false,
      ],
      ["无 HTTP 状态的 invalid_request", adminApiFailureClient("invalid_request").code, false],
      ["错误 HTTP 状态 400", failureAt(400, conflict, "mail-template").code, false],
      ["错误 HTTP 状态 503", failureAt(503, conflict, "mail-template").code, false],
      ["非对象 data true", failureAt(409, fixtureWithData(true), "mail-template").code, false],
      [
        "非对象 data 数组",
        failureAt(409, fixtureWithData(["fixture"]), "mail-template").code,
        false,
      ],
      [
        "可逆拒绝 rename_name_taken",
        failureAt(409, fixture("error-rename-name-taken.json"), "rename-revert").code,
        false,
      ],
      ["可信 409 mail_template_conflict", failureAt(409, conflict, "mail-template").code, true],
    ];
    for (const [label, code, clears] of cases) {
      // 矩阵逐项建立新的未知历史，避免前一例的清除影响后一例。
      await runHistoryCase(code, clears);
      expect(adminApiFailureRejectionAfterReplayCheck(code), label).toBe(clears);
    }
  });

  test("a_preflight_error_is_definite_only_before_dispatch", () => {
    for (const code of ["request_timeout", "network_error", "invalid_response", "http_503"]) {
      const notSent = adminApiFailureNotSentCode(code);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureClient(notSent))).toBe(false);
      expect(adminApiFailureDisplayCode(notSent)).toBe(code);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureClient(code))).toBe(true);
      expect(adminApiFailureOutcomeUnknown(fromHttp(400, notSent))).toBe(true);
    }
  });

  test("a_rejection_with_non_object_data_cannot_clear_unknown_history", () => {
    const body = fixture("error-mail-template-conflict.json");
    const valid = adminApiFailureFromHttpBody(409, body);
    expect(valid.status).toBe(409);
    expect(adminApiFailureOutcomeUnknown(valid)).toBe(false);
    expect(adminApiFailureRejectionAfterReplayCheck(valid.code)).toBe(true);
    const parsed = parseJson(body);
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) throw new Error("错误信封不是合法 JSON");
    const envelope = jsonObject(parsed.value, "错误信封");
    for (const data of [true, ["fixture"]]) {
      envelope.data = data;
      const failure = adminApiFailureFromHttpBody(409, JSON.stringify(envelope));
      expect(adminApiFailureOutcomeUnknown(failure)).toBe(true);
      expect(adminApiFailureRejectionAfterReplayCheck(failure.code)).toBe(false);
    }
  });
});
