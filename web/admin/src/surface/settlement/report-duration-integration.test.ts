// 从实际结算读取入口到报表单元格验证累计领先时长。

import { expect, test } from "bun:test";
import {
  AdminSettlementReportResponseSchema,
  AdminStrictSettlementReportResponseSchema,
} from "@auction/contracts/dto/admin/settlement";
import { decode } from "@auction/contracts/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { fetchAdminSettlement } from "../../api/settlement";
import {
  readAdminFixture,
  requireOk,
  settlementLongHoldBody,
} from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import type { AdminActionFeedback } from "../presentation";
import { createComponent, settle } from "../solid-dom-fixture";
import type { RecoveryConfirmationDraft } from "./recovery";
import { withAdminTree } from "./settlement-tree-fixture.test";

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const REPORT_PATH = `/api/v1/admin/auctions/${AUCTION_ID}/settlement-report`;

async function loadReport(nanos: string) {
  return requireOk(
    await readAdminFixture(settlementLongHoldBody(nanos), REPORT_PATH, (host, signal) =>
      fetchAdminSettlement(host, AUCTION_ID, signal),
    ),
  );
}

async function expectDuration(nanos: string, expected: string) {
  const report = await loadReport(nanos);
  const { AdminSettlementReportView } = await loadRenderModule<typeof import("./report")>(
    `${import.meta.dir}/report.tsx`,
    "AdminSettlementReportView",
  );
  await withAdminTree("/settlement?lot_page_size=10&recovery_page_size=10", async (tree) => {
    tree.mount(() => {
      const reload = createCell(0);
      return createComponent(AdminSettlementReportView, {
        report,
        recovery: null,
        lotPage: createCell(1),
        recoveryPage: createCell(1),
        actionBusy: () => false,
        recoveryRead: { reload, recoveryReload: reload, generation: [0, 0], alive: () => true },
        confirmation: createCell<RecoveryConfirmationDraft | null>(null),
        reason: createCell(""),
        actionFeedback: createCell<AdminActionFeedback | null>(null),
      });
    });
    await settle();
    const cells = tree.elements(
      (node) => node.tag === "td" && node.getAttribute("data-label") === "总时间",
    );
    expect(cells.map((node) => node.textContent)).toEqual([expected]);
  });
}

// 沿用 report.rs 的单元格与 Rust format_held_duration 的秒、分、时、天截断规则。
const SAFE_DURATIONS: readonly [string, string][] = [
  ["0", "0s"],
  ["59999999999", "59s"],
  ["60000000000", "1m"],
  ["3600000000000", "1h 00m"],
  ["86399999999999", "23h 59m"],
  ["86400000000000", "1d 00h 00m"],
  ["9007199254740991", "104d 05h 59m"],
];

test.each(SAFE_DURATIONS)("安全整数 %s 纳秒与 Rust 显示一致（%s）", expectDuration);

// web/README.md 的差异表登记了超过 2^53 纳秒的近似显示，整分钟前 1 纳秒可能显示到下一分钟。
test("9071999999999999 纳秒按浏览器实际显示为 105d 00h 00m", async () => {
  await expectDuration("9071999999999999", "105d 00h 00m");
});

test("结算报告的两种 DTO 都拒绝 bigint 时长", async () => {
  const report = await loadReport("3600000000000");
  const candidate = {
    ...report,
    lots: report.lots.map((lot) => ({ ...lot, leader_held_nanos: 3_600_000_000_000n })),
  };
  for (const schema of [
    AdminSettlementReportResponseSchema,
    AdminStrictSettlementReportResponseSchema,
  ]) {
    expect(decode(schema, report).ok).toBe(true);
    expect(decode(schema, candidate).ok).toBe(false);
  }
});
