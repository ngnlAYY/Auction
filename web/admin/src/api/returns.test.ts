// 按 Rust 的夹具验证退件筛选、摘要、详情与命令回执。

import { expect, test } from "bun:test";
import {
  AdminSettlementReturnCommandResponseSchema,
  AdminSettlementReturnDetailResponseSchema,
  AdminSettlementReturnListResponseSchema,
  type AdminSettlementReturnSummaryResponse,
  AdminSettlementReturnSummaryResponseSchema,
} from "@auction/contracts/dto/admin/returns";
import { decode } from "@auction/contracts/wire";

import { createAdminRequestHarness, TEST_SESSION_BODY } from "./api-request-fixture";
import {
  fetchSettlementReturn,
  fetchSettlementReturns,
  listUrl,
  prepareSettlementReturnActionOnce,
  settlementReturnFiltersDefault,
  submitSettlementReturnAction,
  validCommandResult,
  validDetail,
  validPage,
  validSummary,
} from "./returns";
import { CASE, DELIVERY, returnDetail, returnPage, returnSummary } from "./returns-fixture";
import {
  adminContract,
  checkedContract,
  readAdminFixture,
  requireOk,
} from "./settlement-fixture.test";

const DEFAULT_FILTERS = {
  openOnly: false,
  status: "",
  matchState: "",
  obligationKind: "",
  world: "",
};
const CLOSED_AT = "2026-11-16T02:00:00Z";
const LIST_PATH = "/api/v1/admin/settlement-returns";

test("list_url_accepts_only_contract_filters", async () => {
  const filters = {
    openOnly: true,
    status: "take_unknown",
    matchState: "",
    obligationKind: "buyer_refund",
    world: "isekai",
  };
  expect(listUrl(filters, 2n, 50)).toBe(
    "/api/v1/admin/settlement-returns?page=2&page_size=50&open_only=true&status=take_unknown&obligation_kind=buyer_refund&world=isekai",
  );
  const defaults = settlementReturnFiltersDefault();
  expect(defaults).toEqual(DEFAULT_FILTERS);
  expect(listUrl(defaults, 1n, 20)).toBe("/api/v1/admin/settlement-returns?page=1&page_size=20");
  const fixture = createAdminRequestHarness();
  try {
    const { host } = fixture.registerHost();
    const requests = fixture.installFetch([]);
    for (const invalid of [
      { ...defaults, status: "open" },
      { ...defaults, world: "other" },
      { ...defaults, matchState: "bound" },
    ]) {
      expect(listUrl(invalid, 1n, 20)).toBeNull();
      const result = await fetchSettlementReturns(
        host,
        invalid,
        1n,
        20,
        new AbortController().signal,
      );
      expect(result).toEqual({ ok: false, error: "invalid_request" });
    }
    expect(requests).toEqual([]);
  } finally {
    fixture.cleanup();
  }
});

function summary(value: unknown) {
  return requireOk(decode(AdminSettlementReturnSummaryResponseSchema, value));
}

test("summary_rejects_unknown_names_and_inconsistent_closure", () => {
  expect(validSummary(summary(returnSummary()))).toBe(true);
  for (const [key, value] of Object.entries({
    status: "open",
    asset_location: "in_bot",
    reason_code: "other",
    world_id: "third",
    credits: -1,
    closed_at: CLOSED_AT,
  })) {
    expect(validSummary(summary({ ...returnSummary(), [key]: value })), key).toBe(false);
  }
  const held = { ...returnSummary(), status: "held" };
  expect(validSummary(summary(held))).toBe(false);
  expect(validSummary(summary({ ...held, closed_at: CLOSED_AT }))).toBe(true);
});

test("page_rows_must_match_filters_and_echo", () => {
  const page = (items: readonly AdminSettlementReturnSummaryResponse[], total: number) =>
    requireOk(decode(AdminSettlementReturnListResponseSchema, returnPage(items, total)));
  const open = { ...DEFAULT_FILTERS, openOnly: true };
  const item = summary(returnSummary());
  const one = page([item], 1);
  expect(validPage(one, open, 1n, 20)).toBe(true);
  expect(validPage(page([item, item], 2), open, 1n, 20)).toBe(false);
  expect(validPage(one, { ...DEFAULT_FILTERS, status: "held" }, 1n, 20)).toBe(false);
  expect(validPage(one, { ...DEFAULT_FILTERS, world: "isekai" }, 1n, 20)).toBe(false);
  expect(validPage(one, open, 2n, 20)).toBe(false);
  const reviewed = summary({ ...returnSummary(), status: "reviewed", closed_at: CLOSED_AT });
  expect(validSummary(reviewed)).toBe(true);
  expect(validPage(page([reviewed], 1), open, 1n, 20)).toBe(false);
});

test("detail_requires_partition_order_and_dispatch_shape", () => {
  const { data, firstEvent, secondEvent } = returnDetail();
  const valid = (input: unknown, id = CASE) =>
    validDetail(requireOk(decode(AdminSettlementReturnDetailResponseSchema, input)), id);
  expect(valid(data)).toBe(true);
  expect(valid(data, DELIVERY)).toBe(false);
  expect(valid({ ...data, blockers: data.blockers.slice(0, -1) })).toBe(false);
  expect(
    valid({
      ...data,
      allowed_actions: ["reclaim_and_resend", "reclaim_and_hold", "close_inbox", "recheck"],
    }),
  ).toBe(false);
  expect(
    valid({
      ...data,
      blockers: [{ ...data.blockers[0], blocker: "busy" }, ...data.blockers.slice(1)],
    }),
  ).toBe(false);
  expect(valid({ ...data, events: [firstEvent, { ...secondEvent, case_version: 1 }] })).toBe(false);
  expect(valid({ ...data, dispatches: [{ ...data.dispatches[0], dispatch_no: 2 }] })).toBe(false);
  expect(
    valid({
      ...data,
      custody: { beneficiary: "Seller", credits: 12000, equipment_ids: ["12a"] },
    }),
  ).toBe(false);
  expect(valid({ ...data, recheck_unavailable: true })).toBe(false);
  expect(
    valid({
      ...data,
      summary: { ...data.summary, status: "recheck_pending" },
      recheck_unavailable: true,
    }),
  ).toBe(true);
  expect(valid({ ...data, take_unavailable: true })).toBe(false);
  expect(
    valid({
      ...data,
      summary: { ...data.summary, status: "take_pending" },
      take_unavailable: true,
    }),
  ).toBe(true);
  const takeUnknown = {
    ...data,
    summary: { ...data.summary, status: "take_unknown", plan: "hold" },
  };
  expect(valid({ ...takeUnknown, take_unknown_reason: "assets_claimable" })).toBe(true);
  expect(valid(takeUnknown)).toBe(false);
  expect(valid({ ...takeUnknown, take_unknown_reason: "operator_declared" })).toBe(false);
  expect(valid({ ...data, take_unknown_reason: "mail_missing" })).toBe(false);
});

test("command_result_must_echo_the_request", () => {
  // Rust 的 Uuid::from_u128(7) 用于回执比较，测试的是该层的身份相等条件。
  const commandId = "00000000-0000-0000-0000-000000000007";
  const result = requireOk(
    decode(AdminSettlementReturnCommandResponseSchema, {
      command_id: commandId,
      return_id: CASE,
      action: "close_inbox",
      case_version: 3,
      status: "inbox_closed",
    }),
  );
  expect(validCommandResult(result, commandId, CASE, "close_inbox", 2)).toBe(true);
  expect(
    validCommandResult(result, "00000000-0000-0000-0000-000000000008", CASE, "close_inbox", 2),
  ).toBe(false);
  expect(validCommandResult(result, commandId, DELIVERY, "close_inbox", 2)).toBe(false);
  expect(validCommandResult(result, commandId, CASE, "reclaim_and_hold", 2)).toBe(false);
  expect(validCommandResult(result, commandId, CASE, "close_inbox", 3)).toBe(false);
});

test("settlement_return_read_and_command_contract_fixtures", async () => {
  const returnId = "00000000-0000-4000-8000-000000000024";
  for (const { name, page, size, count } of [
    { name: "settlement-returns", page: 1n, size: 50, count: 1 },
    { name: "settlement-returns-empty-page", page: 2n, size: 1, count: 0 },
  ]) {
    const body = adminContract(name);
    checkedContract(body, AdminSettlementReturnListResponseSchema, "/data/pagination", "total");
    const result = await readAdminFixture(
      body,
      `${LIST_PATH}?page=${page}&page_size=${size}`,
      (host, signal) => fetchSettlementReturns(host, DEFAULT_FILTERS, page, size, signal),
    );
    expect(requireOk(result).returns).toHaveLength(count);
  }

  const detailBody = adminContract("settlement-return");
  checkedContract(detailBody, AdminSettlementReturnDetailResponseSchema, "/data", "events");
  const detail = requireOk(
    await readAdminFixture(detailBody, `${LIST_PATH}/${returnId}`, (host, signal) =>
      fetchSettlementReturn(host, returnId, signal),
    ),
  );
  expect(detail.allowed_actions).toEqual(["acknowledge"]);
  expect(detail.take).toBeNull();

  const body = adminContract("settlement-return-command");
  checkedContract(body, AdminSettlementReturnCommandResponseSchema, "/data", "command_id");
  const commandId = "00000000-0000-4000-8000-000000000007";
  const fixture = createAdminRequestHarness();
  try {
    fixture.installBrowserRequest();
    const { host } = fixture.registerHost();
    const requests = fixture.installFetch([{ body: JSON.stringify(TEST_SESSION_BODY) }, { body }]);
    const submission = prepareSettlementReturnActionOnce({
      returnId,
      expectedVersion: 1,
      action: "acknowledge",
    });
    const result = await submitSettlementReturnAction(
      host,
      commandId,
      submission,
      new AbortController().signal,
    );
    expect(
      requests.map(({ method, url }) => [method, new URL(url, "https://auction.test").href]),
    ).toEqual([
      ["GET", "https://auction.test/api/v1/admin/session?include=server_time"],
      [
        "POST",
        "https://auction.test/api/v1/admin/settlement-returns/00000000-0000-4000-8000-000000000024/handle",
      ],
    ]);
    expect(requests[1]?.body).toBe(
      '{"command_id":"00000000-0000-4000-8000-000000000007","expected_version":1,"action":"acknowledge"}',
    );
    expect(requireOk(result).status).toBe("reviewed");
  } finally {
    fixture.cleanup();
  }
});
