// 本人详情七种投影、服务端授权动作、陈旧版本与重读生命周期。

import { expect, test } from "bun:test";
import type {
  ListingActionV2,
  ListingViewV2,
} from "@auction/contracts/dto/public/consignment/listing-v2";

import {
  ALICE,
  COMMAND_PATH,
  expectedUser,
  isWrite,
  listing,
  mountPrivate,
  OWNED_PATH,
  RECEIPT_V2_PATH,
  receipt,
  requestBody,
  WITHDRAW_PATH,
  withdrawReceipt,
} from "./private-fixture.test";
import {
  controlEvent,
  failure,
  fakeClock,
  fakeFetch,
  LISTING_A,
  LISTING_B,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";

const DETAIL = `/consignment/me/${LISTING_A}`;
// 明显的假标识，不对应任何真实 position 或损失案件。
const POSITION = "00000000-0000-4000-8000-0000000000c3";
const LOSS_CASE = "00000000-0000-4000-8000-0000000000d4";
const REPRICE = "Change asking price";
const WITHDRAW = "Cancel listing";

const CASES = [
  [
    "resolving",
    [
      "Looking up equipment",
      "Attempts made",
      "2 / 6",
      "External lookups are paused",
      "Next lookup at",
      "lookup timed out",
    ],
    false,
  ],
  [
    "lookup_failed",
    ["Equipment lookup failed", "No lookup attempts remain", "rate limiting"],
    false,
  ],
  ["ineligible", ["Listing not permitted", "equipment cannot be traded"], true],
  ["displayed", ["Listed"], true],
  [
    "awaiting_deposit",
    [
      "Awaiting equipment delivery",
      "Do not send equipment",
      "Estimated fee50c",
      "Estimated seller proceeds51c",
    ],
    true,
  ],
  ["closed", ["Closed", "service was disabled"], true],
  ["expired", ["Expired", "lookup deadline has passed"], false],
] as const satisfies readonly (readonly [ListingViewV2["kind"], readonly string[], boolean])[];

test.each(CASES)("本人详情 %s 只展示本视图的数据与原因", async (view, labels, hasEquipment) => {
  const row = listing(
    view,
    view === "awaiting_deposit"
      ? { fee_bps: 5000, estimated_fee: 50, estimated_seller_amount: 51 }
      : {},
  );
  const transport = fakeFetch(() => response(row));
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    expect(page.errors).toEqual([]);
    expect(expectedUser(transport.requests[0])).toBe(ALICE.user_id);
    for (const label of labels) expect(page.text()).toContain(label);
    expect(page.text().includes("Fake Oak Staff")).toBe(hasEquipment);
    expect(page.text().includes("Register again")).toBe(view === "closed" || view === "expired");
    expect(page.text().includes("Do not send equipment")).toBe(view === "awaiting_deposit");
    expect(page.button(REPRICE)).toBeNull();
    expect(page.button(WITHDRAW)).toBeNull();
    expect(page.find((node) => node.attributes.href === "/consignment/new") !== null).toBe(
      view === "closed" || view === "expired",
    );
    expect(transport.requests.map(({ url }) => url.pathname)).toEqual([
      `${OWNED_PATH}/${LISTING_A}`,
    ]);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["resolving", ["withdraw"], false, true],
  ["displayed", ["reprice"], true, false],
  ["displayed", [], false, false],
  ["awaiting_deposit", ["reprice", "withdraw"], true, true],
  ["available", ["reprice", "withdraw"], true, true],
  ["held_off_market", ["withdraw"], false, true],
  ["deposit_accepted", [], false, false],
  ["sale_pending", [], false, false],
  ["withdrawal_pending", [], false, false],
] as const satisfies readonly (readonly [
  ListingViewV2["kind"],
  readonly ListingActionV2[],
  boolean,
  boolean,
])[])("动作由 %s 的 allowed_actions 单独决定", async (view, actions, reprice, withdraw) => {
  const transport = fakeFetch(() => response(listing(view, { allowed_actions: [...actions] })));
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    expect(page.button(REPRICE) !== null).toBe(reprice);
    expect(page.button(WITHDRAW) !== null).toBe(withdraw);
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("改价遇 stale_version 立即撤下旧动作，重读后用新版本和新确认", async () => {
  const refreshed = Promise.withResolvers<Response>();
  let reads = 0;
  let writes = 0;
  const transport = fakeFetch((request) => {
    if (!isWrite(request)) {
      reads += 1;
      if (reads === 1) return response(listing("displayed", { allowed_actions: ["reprice"] }));
      if (reads === 2) return refreshed.promise;
      return response(
        listing("displayed", {
          version: 5,
          price: 115,
          estimated_seller_amount: 115,
          allowed_actions: ["reprice"],
        }),
      );
    }
    writes += 1;
    if (writes === 1) return failure(409, "stale_version");
    return response(
      receipt(String(requestBody(request).command_id), {
        operation: "reprice",
        listing_version: 5,
        state: "displayed",
        disposition: "repriced",
        price: 115,
      }),
    );
  });
  const page = await mountPrivate(DETAIL);
  const typePrice = (value: string) =>
    controlEvent(
      page.find((node) => node.tag === "input"),
      "onInput",
      value,
    );
  try {
    await settle();
    controlEvent(page.button(REPRICE), "onClick");
    typePrice("1e2");
    controlEvent(page.form(REPRICE), "onSubmit");
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
    typePrice("111");
    controlEvent(page.form(REPRICE), "onSubmit");
    await settle();
    expect(page.button(REPRICE)).toBeNull();
    expect(page.form(REPRICE)).toBeNull();
    expect(page.text()).toContain("The listing changed");

    refreshed.resolve(
      response(
        listing("displayed", {
          version: 4,
          price: 110,
          estimated_seller_amount: 110,
          allowed_actions: ["reprice"],
        }),
      ),
    );
    await settle();
    expect(page.text()).toContain("110c");
    expect(page.form(REPRICE)).toBeNull();
    expect(transport.requests.filter(isWrite)).toHaveLength(1);
    controlEvent(page.button(REPRICE), "onClick");
    typePrice("115");
    controlEvent(page.form(REPRICE), "onSubmit");
    await settle();
    const attempts = transport.requests.filter(isWrite);
    expect(attempts).toHaveLength(2);
    expect(requestBody(attempts[0]).expected_version).toBe(3);
    expect(requestBody(attempts[1]).expected_version).toBe(4);
    expect(requestBody(attempts[0]).command_id).not.toBe(requestBody(attempts[1]).command_id);
    expect(page.text()).toContain("115c");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("取消打开既有确认对话框，返回不发包；确认后以原版本关闭", async () => {
  let withdrawn = false;
  const transport = fakeFetch((request) => {
    if (isWrite(request)) {
      withdrawn = true;
      return response(withdrawReceipt(String(requestBody(request).command_id)));
    }
    return response(
      withdrawn ? listing("closed") : listing("resolving", { allowed_actions: ["withdraw"] }),
    );
  });
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    controlEvent(page.button(WITHDRAW), "onClick");
    expect(page.text()).toContain("Cancel this listing?");
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
    controlEvent(page.button("Cancel"), "onClick");
    expect(page.form(WITHDRAW)).toBeNull();
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
    controlEvent(page.button(WITHDRAW), "onClick");
    controlEvent(page.form(WITHDRAW), "onSubmit");
    await settle();
    expect(transport.requests.filter(isWrite)).toHaveLength(1);
    expect(requestBody(transport.requests.find(isWrite)).expected_version).toBe(3);
    expect(page.text()).toContain("Closed");
    expect(page.button(WITHDRAW)).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("解析中每 30 秒重读，离开时取消请求与后续定时器", async () => {
  const clock = fakeClock();
  const delayed = Promise.withResolvers<Response>();
  let reads = 0;
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === STATUS_PATH) return response(status(false));
    reads += 1;
    return reads === 1 ? response(listing("resolving")) : delayed.promise;
  });
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    expect(clock.activeIn(30_000)).toBe(1);
    await clock.advance(29_999);
    expect(reads).toBe(1);
    await clock.advance(1);
    expect(reads).toBe(2);
    const pending = transport.requests.at(-1);
    await page.go("/consignment/new");
    expect(pending?.init?.signal?.aborted).toBe(true);
    delayed.resolve(response(listing("displayed")));
    await clock.advance(60_000);
    expect(reads).toBe(2);
    expect(page.text()).toContain("service is disabled");
    expect(page.button("Submit registration")).toBeNull();
    expect(page.text()).not.toContain("Fake Oak Staff");
  } finally {
    page.dispose();
    transport.restore();
    clock.restore();
  }
});

test("已完成详情不轮询；撤回未决后每 30 秒重读，原样重送的回执落定后停止", async () => {
  const clock = fakeClock();
  let writes = 0;
  let reads = 0;
  const transport = fakeFetch((request) => {
    if (isWrite(request)) {
      writes += 1;
      // 首次结果未知：重送前先查 v2 回执，查不到才原样重送同一条命令。
      return writes === 1
        ? failure(503, "outcome_unknown")
        : response(withdrawReceipt(String(requestBody(request).command_id)));
    }
    if (request.url.pathname.startsWith(COMMAND_PATH)) return failure(404, "command_not_found");
    if (request.url.pathname.startsWith(RECEIPT_V2_PATH)) return failure(404, "command_not_found");
    reads += 1;
    return response(
      writes < 2 ? listing("displayed", { allowed_actions: ["withdraw"] }) : listing("closed"),
    );
  });
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    expect(clock.activeIn(30_000)).toBe(0);
    await clock.advance(30_000);
    expect(reads).toBe(1);
    controlEvent(page.button(WITHDRAW), "onClick");
    controlEvent(page.form(WITHDRAW), "onSubmit");
    await settle();
    expect(clock.activeIn(30_000)).toBe(1);
    await clock.advance(30_000);
    expect(reads).toBeGreaterThan(1);
    expect(writes).toBe(1);
    controlEvent(page.button("Retry original request"), "onClick");
    await settle();
    expect(writes).toBe(2);
    expect(page.text()).toContain("Closed");
    expect(clock.activeIn(30_000)).toBe(0);
  } finally {
    page.dispose();
    transport.restore();
    clock.restore();
  }
});

test("更换详情目标立即撤下旧投影，非本人或不存在的 404 只显示通用提示", async () => {
  const delayed = Promise.withResolvers<Response>();
  const transport = fakeFetch(({ url }) =>
    url.pathname.endsWith(LISTING_A) ? response(listing("displayed")) : delayed.promise,
  );
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    expect(page.text()).toContain("Fake Oak Staff");
    await page.go(`/consignment/me/${LISTING_B}`);
    expect(page.text()).not.toContain("Fake Oak Staff");
    delayed.resolve(failure(404, "listing_not_found"));
    await settle();
    expect(page.text()).toContain("This listing does not exist");
    expect(page.text()).not.toContain("Fake Oak Staff");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("代管件撤回走 v2 路由登记意愿，回执后重读为撤回处理中且不再出现撤回入口", async () => {
  let requested = false;
  const transport = fakeFetch((request) => {
    if (isWrite(request)) {
      requested = true;
      return response({
        schema_version: 2,
        operation: "withdraw",
        disposition: "withdrawal_requested",
        command_id: String(requestBody(request).command_id),
        listing_id: LISTING_A,
        listing_version: 4,
        state: "withdrawal_pending",
        intent_id: "00000000-0000-4000-8000-0000000000c3",
      });
    }
    return response(
      requested
        ? listing("withdrawal_pending", { version: 4 })
        : listing("available", { allowed_actions: ["withdraw"] }),
    );
  });
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    controlEvent(page.button(WITHDRAW), "onClick");
    controlEvent(page.form(WITHDRAW), "onSubmit");
    await settle();
    const writes = transport.requests.filter(isWrite);
    expect(writes).toHaveLength(1);
    expect(writes[0]?.url.pathname).toBe(`${WITHDRAW_PATH}/${LISTING_A}/withdraw`);
    expect(requestBody(writes[0])).toEqual({
      command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
      expected_version: 3,
    });
    expect(page.text()).toContain("Withdrawal pending");
    expect(page.button(WITHDRAW)).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

/** 已在库件直接建撤回代的 202 回执；合同与装备 ID 是固定假值。 */
function acceptedWithdrawal(commandId: string): Response {
  return Response.json(
    {
      code: "accepted",
      message: "",
      data: {
        schema_version: 2,
        operation: "withdraw",
        disposition: "withdrawal_accepted",
        command_id: commandId,
        listing_id: LISTING_A,
        listing_version: 4,
        state: "withdrawal_pending",
        contract_id: "00000000-0000-4000-8000-0000000000c4",
        equipment_id: "00000000-0000-4000-8000-0000000000c5",
      },
    },
    { status: 202 },
  );
}

test("已在库件撤回得 202 withdrawal_accepted：只写一次，提示已受理并重读为撤回处理中", async () => {
  let writes = 0;
  const transport = fakeFetch((request) => {
    if (isWrite(request)) {
      writes += 1;
      return acceptedWithdrawal(String(requestBody(request).command_id));
    }
    return response(
      writes > 0
        ? listing("withdrawal_pending", { version: 4 })
        : listing("available", { allowed_actions: ["withdraw"] }),
    );
  });
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    controlEvent(page.button(WITHDRAW), "onClick");
    controlEvent(page.form(WITHDRAW), "onSubmit");
    await settle();
    expect(writes).toBe(1);
    expect(page.text()).toContain("Withdrawal accepted. The equipment will be mailed back to you.");
    expect(page.text()).toContain("Withdrawal pending");
    expect(page.button(WITHDRAW)).toBeNull();
    expect(page.button("Retry original request")).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("202 回执与命令不符按结果未知：不提示受理，只开放原样重送且请求体逐字节相同", async () => {
  const bodies: string[] = [];
  const transport = fakeFetch((request) => {
    if (isWrite(request)) {
      bodies.push(String(request.init?.body));
      const commandId = String(requestBody(request).command_id);
      // 首次回执属于另一条命令：页面不能据此宣告受理，也不能换新命令 ID。
      return acceptedWithdrawal(
        bodies.length === 1 ? "00000000-0000-4000-8000-0000000000c6" : commandId,
      );
    }
    if (request.url.pathname.startsWith(RECEIPT_V2_PATH)) return failure(404, "command_not_found");
    return response(
      bodies.length > 1
        ? listing("withdrawal_pending", { version: 4 })
        : listing("available", { allowed_actions: ["withdraw"] }),
    );
  });
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    controlEvent(page.button(WITHDRAW), "onClick");
    controlEvent(page.form(WITHDRAW), "onSubmit");
    await settle();
    expect(bodies).toHaveLength(1);
    expect(page.text()).not.toContain("Withdrawal accepted");
    controlEvent(page.button("Retry original request"), "onClick");
    await settle();
    expect(bodies).toHaveLength(2);
    expect(bodies[1]).toBe(bodies[0]);
    expect(page.text()).toContain("Withdrawal accepted. The equipment will be mailed back to you.");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("本人视图不展示止售 hold_id", async () => {
  const transport = fakeFetch(() =>
    response(listing("held_off_market", { allowed_actions: ["withdraw"] })),
  );
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    expect(page.text()).toContain("Held off market");
    expect(page.text().toLowerCase()).not.toContain("hold id");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["有损失案件", LOSS_CASE, true],
  ["没有损失案件", null, false],
] as const)("本人详情%s时损失回执链接是否出现", async (_name, lossCaseId, linked) => {
  const transport = fakeFetch(() =>
    response(listing("closed", { position_id: POSITION, loss_case_id: lossCaseId })),
  );
  const page = await mountPrivate(DETAIL);
  try {
    await settle();
    expect(page.errors).toEqual([]);
    const link = page.find((node) => node.attributes.href === `/consignment/loss/${POSITION}`);
    expect(link !== null).toBe(linked);
    expect(page.text().includes("Loss case")).toBe(linked);
  } finally {
    page.dispose();
    transport.restore();
  }
});
