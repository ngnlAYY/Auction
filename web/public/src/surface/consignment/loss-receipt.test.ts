// 损失案件回执页：只读展示；服务端多给任何字段（如金额）即整体失败，不展示。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { createComponent } from "solid-js";

import { ALICE } from "./private-fixture.test";
import { fakeFetch, response, settle } from "./read-fixture.test";
import { loadPages, mountPage } from "./rights-fixture.test";

const CASE = (
  JSON.parse(
    readFileSync(
      new URL(
        "../../../../../crates/auction-contracts/tests/fixtures/public/consignment-loss-case-seller.json",
        import.meta.url,
      ),
      "utf8",
    ),
  ) as { data: Record<string, unknown> }
).data;
const POSITION = String(CASE.position_id);

async function openLoss() {
  const { LossReceiptPage } = await loadPages();
  return mountPage(
    `/consignment/loss/${POSITION}`,
    () => createComponent(LossReceiptPage, { params: { position_id: POSITION }, query: {} }),
    ALICE,
  );
}

test("案件回执只读展示状态，没有付款或领取入口", async () => {
  const transport = fakeFetch(() => response(CASE));
  const page = await openLoss();
  try {
    await settle();
    expect(page.text()).toContain("Under review");
    expect(page.find((node) => node.tag === "button")).toBeNull();
    expect(transport.requests[0]?.url.pathname).toBe(
      `/api/v2/me/consignment/positions/${POSITION}/loss-case`,
    );
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("回执夹带金额字段时失败关闭，不展示任何案件内容", async () => {
  const transport = fakeFetch(() => response({ ...CASE, payout_amount: 1000 }));
  const page = await openLoss();
  try {
    await settle();
    expect(page.text()).not.toContain("Under review");
    expect(page.text()).not.toContain("1000");
  } finally {
    page.dispose();
    transport.restore();
  }
});
