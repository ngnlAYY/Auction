// Admin 写命令失败分类：拒绝证据、重试边界与 HTTP 状态精确匹配回归。

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { parseJson } from "@auction/web-shared/net/wire";

import { writeFailureFromReply as failureAt } from "./api-request-fixture";
import {
  type AdminApiFailure,
  type AdminWriteEndpoint,
  type AdminWriteFailure,
  adminApiFailureClient,
  adminApiFailureDefiniteRejection,
  adminApiFailureDisplayCode,
  adminApiFailureFromHttpBody,
  adminApiFailureNotSentCode,
  adminApiFailureOutcomeUnknown,
  adminWriteFailureDisplayCode,
  classifyAdminWriteFailure,
  observedResponseStatus,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import { runHistoryCase, runMailTemplateHistory } from "./failure-history-fixture.test";
import { jsonObject } from "./json-fixture";
import { rejectionScope } from "./rejection";

type AssertNever<T extends never> = T;

/** 旧失败投影和普通对象在类型层面不能交给宿主当成写失败证据。 */
export type UntrustedWriteFailure = AssertNever<
  Extract<
    string | AdminApiFailure | { kind: "verified-http"; receipt: { status: 409; code: string } },
    AdminWriteFailure
  >
>;

/** 手工组装的完整证据仍缺少发送器签发的回执句柄。 */
export type UnregisteredWriteEvidence = AssertNever<
  Extract<
    {
      kind: "verified-http";
      receipt: { status: 409; endpoint: "mail-template"; protocolCode: "mail_template_conflict" };
    },
    AdminWriteFailure
  >
>;

const NON_CONFLICT_ERROR_STATUSES = [400, 401, 403, 404, 422, 429, 500, 502, 503, 504] as const;

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

function isReplayChecked(failure: AdminWriteFailure): boolean {
  return classifyAdminWriteFailure(failure) === "replay_checked";
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
  test("delivery_guards_reject_this_attempt_without_clearing_an_earlier_unknown", async () => {
    for (const code of ["delivery_recipient_renamed", "delivery_command_snapshot_unreadable"]) {
      const body = completeBody(code, "投递动作已拒绝");
      const failure = adminApiFailureFromHttpBody(409, body);
      expect(adminApiFailureOutcomeUnknown(failure)).toBe(false);
      expect(isReplayChecked(await failureAt(409, body))).toBe(false);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureFromHttpBody(503, body))).toBe(true);
    }
  });

  test("credential_import_rejections_only_resolve_this_attempt", async () => {
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
      expect(isReplayChecked(await failureAt(status, body)), code).toBe(false);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureFromHttpBody(502, body))).toBe(true);
    }
    const unavailable = adminApiFailureFromHttpBody(
      503,
      '{"code":"storage_unavailable","message":"凭据存储不可用","data":null,"request_id":null}',
    );
    expect(adminApiFailureOutcomeUnknown(unavailable)).toBe(true);
  });

  test("only_complete_error_envelopes_can_be_used_as_rejection_evidence", async () => {
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
      const scope = status === 409 ? "replay_checked" : "this_attempt";
      const completeFailure = await failureAt(status, JSON.stringify(complete), "mail-template");
      expect(classifyAdminWriteFailure(completeFailure)).toBe(scope);
      await runHistoryCase(completeFailure, { display: code, scope });

      for (const key of ["code", "message", "data", "request_id"]) {
        const missing = { ...complete };
        expect(Object.hasOwn(missing, key), `缺少前字段存在 ${key}`).toBe(true);
        delete missing[key];
        const rejected = adminApiFailureFromHttpBody(status, JSON.stringify(missing));
        expect(adminApiFailureOutcomeUnknown(rejected), `缺少 ${key}`).toBe(true);
        expect(adminApiFailureDefiniteRejection(rejected.code), `缺少 ${key}`).toBe(false);
        const failure = await failureAt(status, JSON.stringify(missing), "mail-template");
        expect(failure.kind, `缺少 ${key}`).toBe("outcome-unknown");
        await runHistoryCase(failure, { display: `http_${status}`, scope: "unknown" });
      }

      for (const [key, invalid] of [
        ["code", 409],
        ["message", null],
        ["data", ["fixture"]],
        ["data", true],
        ["request_id", true],
        ["extra", null],
      ] as const) {
        const malformed = { ...complete, [key]: invalid };
        const rejected = adminApiFailureFromHttpBody(status, JSON.stringify(malformed));
        expect(adminApiFailureOutcomeUnknown(rejected), `字段不符 ${key}`).toBe(true);
        expect(adminApiFailureDefiniteRejection(rejected.code), `字段不符 ${key}`).toBe(false);
        const failure = await failureAt(status, JSON.stringify(malformed), "mail-template");
        expect(failure.kind, `字段不符 ${key}`).toBe("outcome-unknown");
        await runHistoryCase(failure, { display: `http_${status}`, scope: "unknown" });
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
      await runHistoryCase(await failureAt(status, JSON.stringify(populated), "mail-template"), {
        display: code,
        scope,
      });

      const positional = JSON.stringify([code, "请求已拒绝", null, null]);
      expect(adminApiFailureOutcomeUnknown(adminApiFailureFromHttpBody(status, positional))).toBe(
        true,
      );
      for (const body of ["<html>错误</html>", "{", positional]) {
        await runHistoryCase(await failureAt(status, body, "mail-template"), {
          display: `http_${status}`,
          scope: "unknown",
        });
      }
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
    const rejected = await failureAt(413, tooLarge, "mail-template");
    expect(classifyAdminWriteFailure(rejected) === "unknown").toBe(false);
    expect(adminWriteFailureDisplayCode(rejected)).toBe("body_too_large");
    expect(isReplayChecked(rejected)).toBe(false);
    await runHistoryCase(rejected, { display: "body_too_large", scope: "this_attempt" });
    await runHistoryCase(writeNotSent("body_too_large"), {
      display: "body_too_large",
      scope: "this_attempt",
    });
    expect(adminApiFailureOutcomeUnknown(fromHttp(413, "http_413"))).toBe(true);
    // 非契约或空正文不能提供拒绝证据，413 只保留 HTTP 兜底并维持未知结果。
    for (const body of ["", "not json", "[]", '{"code":413}']) {
      const fallback = adminApiFailureFromHttpBody(413, body);
      expect(fallback).toEqual({ status: 413, code: "http_413" });
      expect(adminApiFailureOutcomeUnknown(fallback)).toBe(true);
      expect(adminApiFailureDefiniteRejection(fallback.code)).toBe(false);
      const invalid = await failureAt(413, body, "mail-template");
      expect(invalid.kind).toBe("outcome-unknown");
      expect(observedResponseStatus(invalid)).toBe(413);
      await runHistoryCase(invalid, { display: "http_413", scope: "unknown" });
    }
    const unavailable = fromHttp(503, "write_endpoint:mail-template:413:body_too_large");
    expect(adminApiFailureOutcomeUnknown(unavailable)).toBe(true);
    expect(adminApiFailureDefiniteRejection(unavailable.code)).toBe(false);
    await runMailTemplateHistory("413 后同一命令收到可信 409", [
      {
        target: "write",
        status: 413,
        body: '{"code":"body_too_large"}',
        display: "http_413",
        invalidEnvelope: true,
        clears: false,
      },
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

  test("a_first_user_mutation_version_conflict_is_definite", async () => {
    const body = fixture("error-user-state-conflict.json");
    const receipt = adminApiFailureFromHttpBody(409, body);
    const rejection = await failureAt(409, body, "user-mutation");
    expect(observedResponseStatus(rejection)).toBe(409);
    expect(classifyAdminWriteFailure(rejection) === "unknown").toBe(false);
    expect(isReplayChecked(rejection)).toBe(true);
    expect(classifyAdminWriteFailure(rejection) !== "unknown").toBe(true);
    expect(adminWriteFailureDisplayCode(rejection)).toBe("state_conflict");
    expect(adminApiFailureOutcomeUnknown(receipt)).toBe(true);
  });

  test("a_first_rules_or_about_publication_version_conflict_is_definite", async () => {
    for (const endpoint of ["rules-publish", "about-publish"] as AdminWriteEndpoint[]) {
      const body = fixture("error-document-state-conflict.json");
      const rejection = await failureAt(409, body, endpoint);
      expect(observedResponseStatus(rejection)).toBe(409);
      expect(classifyAdminWriteFailure(rejection) === "unknown").toBe(false);
      expect(isReplayChecked(rejection)).toBe(true);
      expect(classifyAdminWriteFailure(rejection) !== "unknown").toBe(true);
      for (const status of [400, 500]) {
        const mismatch = await failureAt(status, body, endpoint);
        expect(classifyAdminWriteFailure(mismatch) === "unknown").toBe(true);
        expect(classifyAdminWriteFailure(mismatch) !== "unknown").toBe(false);
      }
    }
  });

  test("a_server_code_cannot_claim_the_endpoint_or_preflight_marker", () => {
    const tagged = "write_endpoint:user-mutation:409:state_conflict";
    for (const code of [tagged, adminApiFailureNotSentCode("network_error")]) {
      const forged = fromHttp(409, code);
      expect(adminApiFailureOutcomeUnknown(forged)).toBe(true);
      expect(adminApiFailureDefiniteRejection(forged.code)).toBe(false);
    }
  });

  test("reversible_rejections_do_not_resolve_an_earlier_attempt", async () => {
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
      expect(rejectionScope(null, code) === "replay_checked", code).toBe(false);
    }
    for (const name of [
      "error-stale-delivery-state.json",
      "error-stale-delivery-disposition.json",
    ]) {
      const body = fixture(name);
      const failure = adminApiFailureFromHttpBody(409, body);
      expect(failure.code).toBe("stale_delivery_state");
      expect(failure.status).toBe(409);
      expect(adminApiFailureOutcomeUnknown(failure)).toBe(false);
      expect(isReplayChecked(await failureAt(409, body))).toBe(false);
    }
    const conflict = await failureAt(409, fixture("error-rename-name-taken.json"), "rename-revert");
    expect(classifyAdminWriteFailure(conflict) === "unknown").toBe(false);
    expect(isReplayChecked(conflict)).toBe(false);
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
      expect(isReplayChecked(await failureAt(409, body)), code).toBe(true);
      const clientFailure = adminApiFailureClient(rejection.code);
      expect(clientFailure.status, code).toBeNull();
      expect(classifyAdminWriteFailure(writeOutcomeUnknown(clientFailure.code)), code).toBe(
        "unknown",
      );
      await runHistoryCase(writeOutcomeUnknown(code), { display: code, scope: "unknown" });
      await runHistoryCase(writeNotSent(code), { display: code, scope: "this_attempt" });
      await runHistoryCase(await failureAt(409, body), { display: code, scope: "replay_checked" });
      for (const status of NON_CONFLICT_ERROR_STATUSES) {
        const mismatch = adminApiFailureFromHttpBody(status, body);
        expect(mismatch.status, `${code} ${status}`).toBe(status);
        expect(adminApiFailureOutcomeUnknown(mismatch), `${code} ${status}`).toBe(true);
        const failure = await failureAt(status, body);
        expect(classifyAdminWriteFailure(failure), `${code} ${status}`).toBe("unknown");
        await runHistoryCase(failure, { display: `http_${status}`, scope: "unknown" });
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
    expect(isReplayChecked(writeNotSent(adminApiFailureDisplayCode(notSent)))).toBe(false);
    for (const code of ["invalid_alert_command", "stale_credential_state", "rate_limited"]) {
      expect(adminApiFailureDefiniteRejection(code)).toBe(true);
      expect(rejectionScope(null, code) === "replay_checked").toBe(false);
    }
    for (const status of [400, 500]) {
      const mismatch = fromHttp(status, rejection.code);
      expect(adminApiFailureOutcomeUnknown(mismatch)).toBe(true);
      expect(
        isReplayChecked(await failureAt(status, completeBody(rejection.code, "请求已拒绝"))),
      ).toBe(false);
    }
  });

  test("模板重放码配 HTTP 422 保留原请求，随后可信 409 才了结", async () => {
    const body = fixture("error-mail-template-conflict.json");
    await runMailTemplateHistory("mail_template_conflict", [
      { target: "write", status: 422, body, display: "http_422", clears: false },
      { target: "write", status: 409, body, display: "mail_template_conflict", clears: true },
    ]);
  });

  test("pending_command_history_keeps_unknown_until_a_replay_checked_rejection", async () => {
    const conflict = fixture("error-mail-template-conflict.json");
    const cases = [
      [writeOutcomeUnknown("network_error"), "network_error", "unknown"],
      [writeNotSent("request_timeout"), "request_timeout", "this_attempt"],
      [writeNotSent("invalid_request"), "invalid_request", "this_attempt"],
      [writeNotSent("alert_stale"), "alert_stale", "this_attempt"],
      [writeOutcomeUnknown("alert_stale"), "alert_stale", "unknown"],
      [await failureAt(400, conflict, "mail-template"), "http_400", "unknown"],
      [await failureAt(503, conflict, "mail-template"), "http_503", "unknown"],
      [await failureAt(409, fixtureWithData(true), "mail-template"), "http_409", "unknown"],
      [await failureAt(409, fixtureWithData(["fixture"]), "mail-template"), "http_409", "unknown"],
      [
        await failureAt(409, fixture("error-rename-name-taken.json"), "rename-revert"),
        "rename_name_taken",
        "this_attempt",
      ],
      [await failureAt(409, conflict, "mail-template"), "mail_template_conflict", "replay_checked"],
    ] as const;
    for (const [failure, display, scope] of cases) {
      expect(classifyAdminWriteFailure(failure), display).toBe(scope);
      await runHistoryCase(failure, { display, scope });
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

  test("a_rejection_with_non_object_data_cannot_clear_unknown_history", async () => {
    const body = fixture("error-mail-template-conflict.json");
    const valid = adminApiFailureFromHttpBody(409, body);
    expect(valid.status).toBe(409);
    expect(adminApiFailureOutcomeUnknown(valid)).toBe(false);
    expect(isReplayChecked(await failureAt(409, body, "mail-template"))).toBe(true);
    const parsed = parseJson(body);
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) throw new Error("错误信封不是合法 JSON");
    const envelope = jsonObject(parsed.value, "错误信封");
    for (const data of [true, ["fixture"]]) {
      envelope.data = data;
      const failure = adminApiFailureFromHttpBody(409, JSON.stringify(envelope));
      expect(adminApiFailureOutcomeUnknown(failure)).toBe(true);
      expect(isReplayChecked(await failureAt(409, JSON.stringify(envelope), "mail-template"))).toBe(
        false,
      );
    }
  });

  test("坏 UTF-8、畸形信封与伪内部标记保留命令 ID 和正文", async () => {
    const genuinelyRejected = {
      target: "write",
      status: 409,
      body: fixture("error-mail-template-conflict.json"),
      display: "mail_template_conflict",
      clears: true,
    } as const;
    const encoder = new TextEncoder();
    const malformedUtf8 = new Uint8Array([
      ...encoder.encode('{"code":"mail_template_conflict","message":"'),
      0xff,
      ...encoder.encode('","data":null,"request_id":null}'),
    ]);
    const invalidCases = [
      ["坏 UTF-8", malformedUtf8],
      ["坏 JSON", '{"code":"mail_template_conflict"'],
      ["缺 request_id", '{"code":"mail_template_conflict","message":"","data":null}'],
      [
        "重复 code 键",
        '{"code":"outcome_unknown","code":"mail_template_conflict","message":"","data":null,"request_id":null}',
      ],
      ["数组 data", fixtureWithData([])],
      ["伪造端点标记", completeBody("write_endpoint:mail-template:409:mail_template_conflict", "")],
      ["伪造版本化端点标记", completeBody("write_endpoint:user-mutation:409:state_conflict", "")],
      ["伪造未发送标记", completeBody("not_sent:mail_template_conflict", "")],
      ["伪造带重放码的未发送标记", completeBody("not_sent:alert_stale", "")],
      ["伪造本地构造失败码", completeBody("invalid_request", "")],
    ] as const;

    for (const [label, body] of invalidCases) {
      const invalid = {
        target: "write",
        status: 409,
        body,
        display: "http_409",
        invalidEnvelope: true,
        clears: false,
      } as const;
      await runMailTemplateHistory(label, [invalid, genuinelyRejected], invalid);
    }
  });

  test("完整但无法裁定的错误信封保留两种历史", async () => {
    for (const [status, code] of [
      [503, "storage_unavailable"],
      [409, "reconciliation_required"],
      [503, "outcome_unknown"],
      [500, "internal_error"],
      [401, "invalid_credentials"],
      [422, "new_error"],
    ] as const) {
      const failure = await failureAt(status, completeBody(code, "请求已拒绝"));
      expect(failure.kind, code).toBe("verified-http");
      expect(observedResponseStatus(failure), code).toBe(status);
      expect(classifyAdminWriteFailure(failure), code).toBe("unknown");
      await runHistoryCase(failure, { display: code, scope: "unknown" });
    }
    for (const code of ["network_error", "invalid_response", "http_400", "http_413", "http_503"]) {
      await runHistoryCase(writeOutcomeUnknown(code), { display: code, scope: "unknown" });
    }
  });
});
