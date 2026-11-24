// 本人详情七种投影、服务端授权动作、陈旧版本与重读生命周期。

import { expect, test } from "bun:test";
import type { ListingAction, ListingView } from "@auction/contracts/dto/public/consignment";

import {
  ALICE,
  COMMAND_PATH,
  expectedUser,
  isWrite,
  listing,
  mountPrivate,
  OWNED_PATH,
  receipt,
  requestBody,
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
] as const satisfies readonly (readonly [ListingView["kind"], readonly string[], boolean])[];

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
] as const satisfies readonly (readonly [
  ListingView["kind"],
  readonly ListingAction[],
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
      return response(
        receipt(String(requestBody(request).command_id), {
          operation: "withdraw",
          listing_version: 4,
          state: "closed",
          disposition: "closed",
          price: null,
        }),
      );
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

test("已完成详情不轮询；发生未决命令后每 30 秒重读，回执落定后停止", async () => {
  const clock = fakeClock();
  let commandId = "";
  let checks = 0;
  let reads = 0;
  const transport = fakeFetch((request) => {
    if (isWrite(request)) {
      commandId = String(requestBody(request).command_id);
      return failure(503, "outcome_unknown");
    }
    if (request.url.pathname.startsWith(COMMAND_PATH)) {
      checks += 1;
      return response({
        code: "ok",
        http_status: 200,
        receipt: receipt(commandId, {
          operation: "withdraw",
          listing_version: 4,
          state: "closed",
          disposition: "closed",
          price: null,
        }),
      });
    }
    reads += 1;
    return response(
      checks === 0 ? listing("displayed", { allowed_actions: ["withdraw"] }) : listing("closed"),
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
    expect(checks).toBe(1);
    expect(reads).toBeGreaterThan(1);
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
