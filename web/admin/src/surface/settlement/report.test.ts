// 结算报告移动卡片的成对字段标签、配额与时长展示。

import { expect, test } from "bun:test";
import { AdminSettlementReportResponseSchema } from "@auction/contracts/dto/admin/settlement";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { type AdminSettlementReport, fetchAdminSettlement } from "../../api/settlement";
import {
  adminContract,
  readAdminFixture,
  requireOk,
  settlementLongHoldBody,
} from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import type { AdminActionFeedback } from "../presentation";
import { createComponent, settle, TestElement } from "../solid-dom-fixture";
import type { RecoveryConfirmationDraft } from "./recovery";
import { cardLabels, requireElement, withAdminTree } from "./settlement-tree-fixture.test";

function textNodes(node: unknown): string[] {
  if (typeof node === "string") {
    return [node];
  }
  if (typeof node === "function") {
    return textNodes(node());
  }
  if (Array.isArray(node)) {
    return node.flatMap(textNodes);
  }
  if (node instanceof TestElement) {
    return node.tag === "#text" ? [node.text] : node.nodes.flatMap(textNodes);
  }
  return [];
}

async function withReportBody(
  report: AdminSettlementReport,
  check: (body: TestElement) => void,
): Promise<void> {
  const { AdminSettlementReportView } = await loadRenderModule<
    Pick<typeof import("./report"), "AdminSettlementReportView">
  >(`${import.meta.dir}/report.tsx`, "AdminSettlementReportView");
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
    const body = requireElement(
      tree.find((node) => node.tag === "tbody"),
      "结算报告表体",
    );
    expect(body.querySelectorAll("tr")).toHaveLength(1);
    check(body);
  });
}

function readBoundaryReport() {
  const auctionId = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
  return readAdminFixture(
    settlementLongHoldBody("9071999999999999"),
    "/api/v1/admin/auctions/0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b/settlement-report",
    (host, signal) => fetchAdminSettlement(host, auctionId, signal),
  );
}

test("report_cards_label_each_value_in_the_paired_columns", async () => {
  const { data } = requireOk(
    decode(
      apiResponseSchema(AdminSettlementReportResponseSchema),
      requireOk(parseJson(adminContract("settlement-report"))),
    ),
  );
  // 保留 Rust 行数据的其它字段，领先纳秒从同一词法值的实际 GET 取得。
  const first = requireOk(await readBoundaryReport()).lots[0];
  if (first === undefined) {
    throw new Error("领先时长样本缺少拍品");
  }
  const report = {
    ...data,
    settlement: null,
    lots: [
      {
        id: "00000000-0000-0000-0000-000000000001",
        lot_number: 1,
        status: "sold",
        equipment_name: "Legendary Phase Robe",
        equipment_category: "Cloth",
        game_equipment_url: "https://example.invalid/equip/123",
        seller: "seller",
        winner: "winner",
        final_price: 300000,
        listed_price: 298800,
        buyer_amount: 300000,
        seller_amount: 300000,
        tax_amount: 0,
        compensation_amount: 0,
        hold_ppm: 300000,
        adjustment_ppm: -4000,
        leader_held_nanos: first.leader_held_nanos,
        adjustment: { policy: "hold_curve_v2" },
      },
    ],
  };
  await withReportBody(report, (body) => {
    expect(cardLabels(body)).toEqual(
      new Set([
        "编号 ID",
        "装备",
        "卖家",
        "得主",
        "成交价",
        "持有比例",
        "调整比例",
        "总时间",
        "名义折扣 / 惩罚",
        "挂牌价",
        "补偿",
        "买家应付",
        "卖家应收",
      ]),
    );
    const text = textNodes(body);
    expect(text.some((value) => value.includes("名义折扣 −1,200"))).toBe(true);
    expect(text.filter((value) => value === "300K")).toHaveLength(3);
  });
});

// 展示时长的近似精度见 web/README.md「样式与译文」差异表（AW3-3）。
test("report_from_get_displays_105_days_after_rounding_one_nanosecond", async () => {
  const report = requireOk(await readBoundaryReport());
  await withReportBody(report, (body) => {
    const heldTime = requireElement(
      body.querySelectorAll("td").find((node) => node.getAttribute("data-label") === "总时间") ??
        null,
      "总时间单元格",
    );
    expect(heldTime.textContent).toBe("105d 00h 00m");
  });
});
