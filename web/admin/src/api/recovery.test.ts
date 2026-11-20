// 恢复报告读取的发送归属扩展、闭集与契约样本。

import { expect, test } from "bun:test";
import { AdminRecoveryReportResponseSchema } from "@auction/contracts/dto/admin/recovery";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";

import { parseJsonObject, setAt } from "./api-request-fixture";
import { fetchAdminRecovery } from "./recovery";
import {
  adminContract,
  checkedContract,
  readAdminFixture,
  requireOk,
} from "./settlement-fixture.test";

const AUCTION_ID = "00000000-0000-4000-8000-000000000002";
const REPORT_PATH =
  "/api/v1/admin/auctions/00000000-0000-4000-8000-000000000002/recovery-report?include=send_attribution";

function readReport(body: string) {
  return readAdminFixture(body, REPORT_PATH, (host, signal) =>
    fetchAdminRecovery(host, AUCTION_ID, signal),
  );
}

test("recovery_report_requests_send_attribution_and_checks_the_closed_set", async () => {
  const report = adminContract("recovery-report-send-attribution");
  const valid = requireOk(await readReport(report));
  expect(valid.recoveries[0]?.send_attribution_blocker).toBe("send_without_sent_mid");
  const invalid = parseJsonObject(report, "恢复报告");
  setAt(invalid, "/data/recoveries/0/send_attribution_blocker", "operator_declared");
  // 与 Rust 一样，闭集检查发生在 DTO 解码之后。
  expect(decode(apiResponseSchema(AdminRecoveryReportResponseSchema), invalid).ok).toBe(true);
  expect(await readReport(JSON.stringify(invalid))).toEqual({
    ok: false,
    error: "invalid_response",
  });
});

test("recovery_report_contract_fixtures", async () => {
  for (const [name, expected] of new Map([
    ["recovery-report", null],
    ["recovery-report-send-attribution", "send_without_sent_mid"],
  ])) {
    const body = adminContract(name);
    checkedContract(
      body,
      AdminRecoveryReportResponseSchema,
      "/data/recoveries/0",
      "custody_proof_kind",
    );
    const report = requireOk(await readReport(body));
    const recovery = report.recoveries[0];
    if (recovery === undefined) {
      throw new Error("样本缺少恢复记录");
    }
    expect(recovery.custody_proof_kind).toBe("delivery_recovery_take_succeeded");
    expect(recovery.custody_proof_observed_at).toBe("2025-11-15T09:45:40.110000Z");
    expect(recovery.actions[0]?.kind).toBe("buyer_blacklist");
    expect(recovery.actions[1]?.kind).toBe("reclaim_equipment_take");
    expect(recovery.allowed_commands).toEqual([]);
    expect(recovery.send_attribution_blocker).toBe(expected);
    expect(recovery.blocking_code).toBe("recovery_evidence_mismatch");
  }
});
