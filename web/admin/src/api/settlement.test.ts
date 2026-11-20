// 结算报告的可空字段、后端样本和展示时长的 i64 边界。

import { expect, test } from "bun:test";
import { AdminStrictSettlementReportResponseSchema } from "@auction/contracts/dto/admin/settlement";
import { decode } from "@auction/contracts/wire";
import { formatHeldDuration } from "@auction/web-shared/ui/format";

import { removeAt } from "./api-request-fixture";
import { fetchAdminSettlement } from "./settlement";
import {
  adminContract,
  readAdminFixture,
  requireOk,
  settlementLongHoldBody,
} from "./settlement-fixture.test";

const AUCTION_ID = "00000000-0000-4000-8000-000000000001";
const CONTRACT_AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

function auction() {
  return {
    id: AUCTION_ID,
    world_id: "main",
    sequence: 1,
    status: "settled",
    preview_scheduled_at: "2026-11-01T00:00:00Z",
    scheduled_open_at: "2026-11-02T00:00:00Z",
    preview_opened_at: "2026-11-01T00:00:00Z",
    preview_manual_open: false,
    preview_revision: 0,
    opened_at: "2026-11-02T00:00:00Z",
    final_phase_at: "2026-11-03T00:00:00Z",
    settled_at: "2026-11-03T01:00:00Z",
    cancelled_at: null,
    cancelled_reason: null,
    cancelled_by: null,
    state_version: 5,
    created_at: "2026-10-31T00:00:00Z",
    updated_at: "2026-11-03T01:00:00Z",
  };
}

function lot() {
  return {
    id: "00000000-0000-4000-8000-000000000002",
    lot_number: 1,
    status: "unsold",
    equipment_name: "Legendary Ethereal Rapier",
    equipment_category: "One-handed Weapon",
    game_equipment_url: "https://hentaiverse.org/equip/123",
    seller: null,
    winner: null,
    final_price: 0,
    listed_price: 100,
    buyer_amount: 0,
    seller_amount: 0,
    tax_amount: 0,
    compensation_amount: 0,
    hold_ppm: 0,
    adjustment_ppm: 0,
    leader_held_nanos: 0,
    adjustment: {},
  };
}

function report(lots: readonly unknown[]) {
  return { auction: auction(), lots, settlement: null };
}

function readReport(body: string, auctionId: string) {
  return readAdminFixture(
    body,
    `/api/v1/admin/auctions/${auctionId}/settlement-report`,
    (host, signal) => fetchAdminSettlement(host, auctionId, signal),
  );
}

test("explicit_null_settlement_and_parties_decode_as_none", async () => {
  const data = report([lot()]);
  expect(decode(AdminStrictSettlementReportResponseSchema, data).ok).toBe(true);
  const decoded = requireOk(
    await readReport(JSON.stringify({ code: "ok", message: "", data }), AUCTION_ID),
  );
  expect(decoded.settlement).toBeNull();
  expect(decoded.lots[0]?.seller).toBeNull();
  expect(decoded.lots[0]?.winner).toBeNull();
});

test("missing_nullable_settlement_fields_fail_closed", () => {
  const missingSettlement = report([lot()]);
  expect(decode(AdminStrictSettlementReportResponseSchema, missingSettlement).ok).toBe(true);
  expect(removeAt(missingSettlement, "/settlement")).toBeNull();
  expect(decode(AdminStrictSettlementReportResponseSchema, missingSettlement).ok).toBe(false);
  for (const key of ["seller", "winner"]) {
    const missingParty = report([lot()]);
    expect(removeAt(missingParty, `/lots/0/${key}`)).toBeNull();
    expect(decode(AdminStrictSettlementReportResponseSchema, missingParty).ok, `缺少 ${key}`).toBe(
      false,
    );
  }
});

test("settlement_reports_decode_backend_contract_fixtures", async () => {
  for (const [name, sold] of new Map([
    ["settlement-report", true],
    ["settlement-report-unsold", false],
  ])) {
    const report = requireOk(await readReport(adminContract(name), CONTRACT_AUCTION_ID));
    expect(report.auction.status).toBe("settled");
    const first = report.lots[0];
    if (first === undefined) {
      throw new Error("结算报告样本缺少拍品");
    }
    expect(first.winner !== null).toBe(sold);
  }
});

async function firstHeldNanos(body: string): Promise<number> {
  const report = requireOk(await readReport(body, CONTRACT_AUCTION_ID));
  expect(report.auction.status).toBe("settled");
  const first = report.lots[0];
  if (first === undefined) {
    throw new Error("结算报告样本缺少拍品");
  }
  return first.leader_held_nanos;
}

test("settlement_report_decodes_full_nonnegative_i64_held_duration", async () => {
  const sample = await firstHeldNanos(adminContract("settlement-report-long-hold"));
  expect(sample).toBe(9_072_000_000_000_000);
  expect(formatHeldDuration(sample)).toBe("105d 00h 00m");

  const twoToThe53rd = await firstHeldNanos(settlementLongHoldBody("9007199254740992"));
  expect(twoToThe53rd).toBe(9_007_199_254_740_992);
  expect(formatHeldDuration(twoToThe53rd)).toBe("104d 05h 59m");

  for (const invalid of ["-1", "1.5", '"1"']) {
    const result = await readReport(settlementLongHoldBody(invalid), CONTRACT_AUCTION_ID);
    expect(result, invalid).toEqual({ ok: false, error: "invalid_response" });
  }

  const noTime = settlementLongHoldBody("0");
  const buyerAmount = '"buyer_amount": 2000';
  expect(noTime.split(buyerAmount)).toHaveLength(2);
  expect(await firstHeldNanos(noTime)).toBe(0);
  const withUnsafeAmount = noTime.replace(buyerAmount, '"buyer_amount": 9007199254740992');
  expect(withUnsafeAmount).not.toBe(noTime);
  expect(await readReport(withUnsafeAmount, CONTRACT_AUCTION_ID)).toEqual({
    ok: false,
    error: "invalid_response",
  });
});

// 展示时长的近似精度见 web/README.md「样式与译文」差异表（AW3-3）。
test("settlement_report_accepts_and_formats_i64_max_held_duration", async () => {
  const nanos = await firstHeldNanos(settlementLongHoldBody("9223372036854775807"));
  expect(formatHeldDuration(nanos)).toBe("106751d 23h 47m");
});
