// 从真实超期明细行读取分类内编号生成的拍品代码。

import { expect, test } from "bun:test";
import { createComponent } from "solid-js";
import { completePayload, dashboardSnapshot } from "../../../api/dashboard/dashboard-fixture";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { findAll } from "../../solid-dom-fixture";
import { oneT11Element, withT11View } from "../t11-solid-fixture.test";
import { deriveDashboard } from "./derived";

test("overdue_lot_code_uses_the_category_number", async () => {
  const { FulfillmentPanel: Panel } = await loadRenderModule<typeof import("./fulfillment")>(
    new URL("./fulfillment.tsx", import.meta.url).pathname,
    "FulfillmentPanel",
  );
  const payload = completePayload();
  payload.fulfillment.data.collection_overdue_items = [
    {
      obligation_id: "00000000-0000-0000-0000-000000000010",
      obligation_kind: "seller_payout",
      world: "main",
      auction_sequence: 4,
      lot_number: 2,
      equipment_category: "Shield",
      equipment_name: "Radiant Buckler",
      recipient: "collection-seller",
      sent_at: "2026-01-01T00:00:00.000000Z",
      age_seconds: 691_200,
      last_read_failed: false,
    },
  ];
  const snapshot = dashboardSnapshot(payload);
  await withT11View(
    () => createComponent(Panel, { snapshot, derived: deriveDashboard(snapshot) }),
    (tree) => {
      const row = oneT11Element(
        findAll(
          tree,
          (element) => element.tag === "tr" && element.textContent.includes("Radiant Buckler"),
        ),
        "长期未领取记录",
      );
      const code = row.querySelector("td");
      expect(code?.nodes.at(-1)?.()).toBe("Shd02");
    },
  );
});
