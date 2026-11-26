// 登记表单的能力、验证、报价、私密输入和提交接线。

import { expect, test } from "bun:test";
import type { OwnedListingV2 } from "@auction/contracts/dto/public/consignment/listing-v2";

import {
  ALICE,
  acceptedReceipt,
  assertNoSecret,
  EQUIPMENT_URL,
  expectedUser,
  FIXTURE_KEY,
  fill,
  ISEKAI_URL,
  isWrite,
  listing,
  memory,
  mountPrivate,
  OWNED_PATH,
  receipt,
  requestBody,
  submitRegistration,
  WITHDRAW_PATH,
  withdrawReceipt,
} from "./private-fixture.test";
import {
  controlEvent,
  failure,
  fakeClock,
  fakeFetch,
  LISTING_A,
  LISTINGS_PATH,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";

const NEW = "/consignment/new";
const SUBMIT = "Submit registration";
const REPRICE = "Change asking price";
const WITHDRAW = "Cancel listing";
test("匿名登记沿用登录后返回原页，不启动本人请求", async () => {
  const transport = fakeFetch(() => response(status(true)));
  const page = await mountPrivate(NEW, null);
  try {
    await settle();
    expect(page.errors).toEqual([]);
    expect(
      page.find((node) => node.attributes.href === "/login?return_to=%2Fconsignment%2Fnew"),
    ).not.toBeNull();
    expect(page.input("password")).toBeNull();
    expect(transport.requests).toHaveLength(0);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("合法装备链接只投影世界和字符串编号，报价为预计值并向下取整", async () => {
  const transport = fakeFetch(() => response({ ...status(true), fee_bps: 5000 }));
  const page = await mountPrivate(NEW);
  try {
    await settle();
    expect(page.errors).toEqual([]);
    expect(page.button(SUBMIT)?.attributes.disabled).toBe(true);
    fill(page, "consigned", `<${ISEKAI_URL}>。`);
    await settle();
    expect(page.text()).toContain("Equipment ID42");
    expect(page.text()).toContain("Isekai");
    expect(page.text()).toContain("Estimated fee50c");
    expect(page.text()).toContain("Estimated seller proceeds51c");
    expect(page.text()).toContain("hvdata.gisadan.dev");
    expect(page.text().includes(FIXTURE_KEY)).toBe(false);
    expect(page.button(SUBMIT)?.attributes.disabled).toBe(false);
    assertNoSecret(page);
    fill(page, "direct");
    expect(page.text()).toContain("Estimated fee0K");
    expect(page.text()).toContain("Estimated seller proceeds101c");
    expect(page.button(SUBMIT)?.attributes.disabled).toBe(false);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("不可登记的模式、世界、链接与价格不能绕过提交处理器", async () => {
  const transport = fakeFetch(() =>
    response({ ...status(true), registration_modes: ["direct"], registration_worlds: ["main"] }),
  );
  const page = await mountPrivate(NEW);
  try {
    await settle();
    expect(
      page.find((node) => node.tag === "option" && node.attributes.value === "consigned"),
    ).toBeNull();
    const invalid = [
      ["consigned", EQUIPMENT_URL, "101"],
      ["direct", ISEKAI_URL, "101"],
      ["direct", EQUIPMENT_URL.replace("/42/", "/00042/"), "101"],
      ["direct", EQUIPMENT_URL.replace("hentaiverse.org", "fake.invalid"), "101"],
      ["direct", EQUIPMENT_URL.replace(FIXTURE_KEY, "not-a-key"), "101"],
      ...["", "0", "01", "1e2", "1.1", "-1", "1000000001", "１０"].map((price) => [
        "direct",
        EQUIPMENT_URL,
        price,
      ]),
    ];
    for (const [mode, url, price] of invalid) {
      fill(page, mode, url, price);
      expect(page.button(SUBMIT)?.attributes.disabled).toBe(true);
      submitRegistration(page);
    }
    await settle();
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
    fill(page, "direct", EQUIPMENT_URL, "1000000000");
    expect(page.button(SUBMIT)?.attributes.disabled).toBe(false);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["关闭", () => response(status(false)), "consignment market is disabled"],
  ["只读", () => response(status(true, false)), "consignment market is temporarily unavailable"],
  [
    "失败",
    () => failure(503, "consignment_unavailable"),
    "consignment market is temporarily unavailable",
  ],
] as const)("能力%s显示原因并且不提交", async (_name, capability, message) => {
  const transport = fakeFetch(capability);
  const page = await mountPrivate(NEW);
  try {
    await settle();
    expect(page.text()).toContain(message);
    expect(page.button(SUBMIT)).toBeNull();
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("首读能力未完成时不暴露写入口，完成后可填写", async () => {
  const capability = Promise.withResolvers<Response>();
  const transport = fakeFetch(() => capability.promise);
  const page = await mountPrivate(NEW);
  try {
    await settle();
    expect(page.text()).toContain("Loading");
    expect(page.button(SUBMIT)).toBeNull();
    capability.resolve(response(status(true)));
    await settle();
    expect(page.button(SUBMIT)?.attributes.disabled).toBe(true);
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["main", "direct", EQUIPMENT_URL, 0],
  ["isekai", "direct", ISEKAI_URL, 0],
  ["main", "consigned", EQUIPMENT_URL, 500],
  ["isekai", "consigned", ISEKAI_URL, 500],
] as const)("%s / %s 同一挂单从登记 202 到改价、取消后只读", async (world, mode, url, fee) => {
  const clock = fakeClock();
  const lookup = Promise.withResolvers<Response>();
  const reads: OwnedListingV2[] = [];
  const fixture: { listing: OwnedListingV2 | null } = { listing: null };
  const detailPath = `${OWNED_PATH}/${LISTING_A}`;
  const worldLabel = world === "main" ? "Main" : "Isekai";
  const modeLabel = mode === "direct" ? "Direct sale" : "Consignment";
  const active = mode === "direct" ? "displayed" : "awaiting_deposit";
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return response(status(true));
    if (pathname === LISTINGS_PATH) {
      const body = requestBody(request);
      expect(typeof body.command_id).toBe("string");
      expect(body.equipment_url === url).toBe(true);
      expect([body.mode, body.price, body.expected_fee_bps]).toEqual([mode, 101, fee]);
      fixture.listing = listing("resolving", {
        version: 1,
        world_id: body.equipment_url === ISEKAI_URL ? "isekai" : "main",
        mode: body.mode === "consigned" ? "consigned" : "direct",
        fee_bps: fee,
        estimated_fee: fee === 0 ? 0 : 5,
        estimated_seller_amount: fee === 0 ? 101 : 96,
      });
      return acceptedReceipt(receipt(String(body.command_id)));
    }
    const current = fixture.listing;
    if (current === null) throw new Error("尚无登记记录");
    if (pathname === detailPath) {
      reads.push(current);
      return reads.length === 1 ? lookup.promise : response(current);
    }

    const body = requestBody(request);
    expect(body.expected_version).toBe(current.version);
    if (pathname === `${LISTINGS_PATH}/${current.listing_id}/price`) {
      expect(body.price).toBe(203);
      fixture.listing = {
        ...current,
        version: 3,
        price: 203,
        estimated_fee: fee === 0 ? 0 : 10,
        estimated_seller_amount: fee === 0 ? 203 : 193,
      };
      return response(
        receipt(String(body.command_id), {
          operation: "reprice",
          listing_id: fixture.listing.listing_id,
          listing_version: fixture.listing.version,
          state: active,
          disposition: "repriced",
          price: fixture.listing.price,
        }),
      );
    }

    expect(pathname).toBe(`${WITHDRAW_PATH}/${current.listing_id}/withdraw`);
    if (current.view.kind !== active) throw new Error("取消前缺少发布投影");
    fixture.listing = {
      ...current,
      version: 4,
      stored_state: "closed",
      effective_state: "closed",
      effective_reason: "seller_cancelled",
      allowed_actions: [],
      view: { kind: "closed", reason: "seller_cancelled", equipment: current.view.equipment },
    };
    return response(withdrawReceipt(String(body.command_id), fixture.listing.version));
  });
  const page = await mountPrivate(NEW);
  try {
    await settle();
    fill(page, mode, url);
    assertNoSecret(page);
    submitRegistration(page);
    assertNoSecret(page);
    await settle();
    expect(page.location.pathname).toBe(`/consignment/me/${LISTING_A}`);
    expect(transport.requests.at(-1)?.url.pathname).toBe(detailPath);
    expect(expectedUser(transport.requests.find(isWrite))).toBe(ALICE.user_id);
    expect(page.text()).toContain("Loading");
    assertNoSecret(page);
    const current = fixture.listing;
    if (current === null) throw new Error("首读前缺少登记记录");
    lookup.resolve(response(current));
    await settle();
    expect(page.text()).toContain("Looking up equipment");
    expect(page.text()).toContain(`World${worldLabel}`);
    expect(page.text()).toContain(`Listing mode${modeLabel}`);

    fixture.listing = {
      ...current,
      version: 2,
      stored_state: active,
      effective_state: active,
      allowed_actions: ["reprice", "withdraw"],
      view: listing(active).view,
    };
    await clock.advance(30_000);
    expect(page.text()).toContain(mode === "direct" ? "Listed" : "Awaiting equipment delivery");
    expect(page.text().includes("Do not send equipment")).toBe(mode === "consigned");
    expect(page.text()).toContain(`World${worldLabel}`);
    expect(page.text()).toContain(`Listing mode${modeLabel}`);
    expect(page.text()).toContain("101c");
    expect(page.button(REPRICE)?.attributes.disabled).toBe(false);
    expect(page.button(WITHDRAW)?.attributes.disabled).toBe(false);

    controlEvent(page.button(REPRICE), "onClick");
    controlEvent(
      page.find((node) => node.attributes.inputmode === "numeric"),
      "onInput",
      "203",
    );
    controlEvent(page.form(REPRICE), "onSubmit");
    await settle();
    expect(reads.map((row) => row.version)).toEqual([1, 2, 3]);
    expect(page.text()).toContain("203c");
    expect(page.text()).toContain(`Estimated seller proceeds${fee === 0 ? 203 : 193}c`);
    expect(page.text()).toContain(`World${worldLabel}`);
    expect(page.text()).toContain(`Listing mode${modeLabel}`);
    expect(page.text().includes("Do not send equipment")).toBe(mode === "consigned");

    controlEvent(page.button(WITHDRAW), "onClick");
    expect(page.text()).toContain("Cancel this listing?");
    controlEvent(page.form(WITHDRAW), "onSubmit");
    await settle();
    expect(page.text()).toContain("The seller cancelled the listing.");
    expect(page.text()).toContain(`World${worldLabel}`);
    expect(page.text()).toContain(`Listing mode${modeLabel}`);
    expect(page.text()).toContain("203c");
    expect(page.button(REPRICE)).toBeNull();
    expect(page.button(WITHDRAW)).toBeNull();
    expect(page.find((node) => node.attributes.href === NEW)).not.toBeNull();
    expect(page.location.pathname).toBe(`/consignment/me/${LISTING_A}`);
    expect(
      reads.map((row) => [row.listing_id, row.world_id, row.mode, row.version, row.view.kind]),
    ).toEqual([
      [LISTING_A, world, mode, 1, "resolving"],
      [LISTING_A, world, mode, 2, active],
      [LISTING_A, world, mode, 3, active],
      [LISTING_A, world, mode, 4, "closed"],
    ]);
    const writes = transport.requests.filter(isWrite);
    expect(writes.map((request) => request.url.pathname)).toEqual([
      LISTINGS_PATH,
      `${LISTINGS_PATH}/${LISTING_A}/price`,
      `${WITHDRAW_PATH}/${LISTING_A}/withdraw`,
    ]);
    expect(writes.map((request) => requestBody(request).expected_version)).toEqual([
      undefined,
      2,
      3,
    ]);
    expect(new Set(writes.map((request) => requestBody(request).command_id)).size).toBe(3);
    expect(writes.map(expectedUser)).toEqual([ALICE.user_id, ALICE.user_id, ALICE.user_id]);
    expect(page.errors).toEqual([]);
    assertNoSecret(page);
  } finally {
    page.dispose();
    transport.restore();
    clock.restore();
  }
});

test("费率已变时先重读能力，再显示新报价并由玩家重新提交", async () => {
  const refreshed = Promise.withResolvers<Response>();
  let statusReads = 0;
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === STATUS_PATH) {
      statusReads += 1;
      return statusReads === 1 ? response(status(true)) : refreshed.promise;
    }
    return failure(409, "fee_changed");
  });
  const page = await mountPrivate(NEW, ALICE, memory());
  try {
    await settle();
    fill(page);
    submitRegistration(page);
    await settle();
    expect(page.text()).toContain("The fee rate changed");
    expect(page.button(SUBMIT)).toBeNull();
    expect(transport.requests.filter(isWrite)).toHaveLength(1);
    refreshed.resolve(response({ ...status(true), fee_bps: 5000 }));
    await settle();
    expect(page.text()).toContain("Estimated fee50c");
    expect(page.button(SUBMIT)?.attributes.disabled).toBe(true);
    fill(page);
    submitRegistration(page);
    await settle();
    const [original, reviewed] = transport.requests.filter(isWrite);
    expect(requestBody(original).expected_fee_bps).toBe(500);
    expect(requestBody(reviewed).expected_fee_bps).toBe(5000);
    expect(requestBody(original).command_id).not.toBe(requestBody(reviewed).command_id);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  [400, "invalid_equipment_url", "Enter an equipment URL"],
  [400, "invalid_price", "Enter a valid whole-number asking price"],
  [400, "invalid_command_id", "Auction data is temporarily unavailable"],
  [400, "invalid_listing_request", "Auction data is temporarily unavailable"],
  [400, "invalid_expected_user_id", "Auction data is temporarily unavailable"],
  [401, "unauthenticated", "Your session expired"],
  [403, "origin_forbidden", "Auction data is temporarily unavailable"],
  [403, "participant_ineligible", "currently ineligible"],
  [404, "listing_not_found", "does not exist or is no longer displayed"],
  [404, "consignment_disabled", "consignment market is disabled"],
  [409, "already_open", "already have an active registration"],
  [409, "equipment_busy", "outstanding auction obligations"],
  [409, "history_unproven", "insufficient proof"],
  [409, "history_owner_mismatch", "last owner verified"],
  [409, "history_alias_conflict", "conflicting records"],
  [409, "world_unavailable", "world is temporarily closed"],
  [409, "stale_version", "listing changed"],
  [409, "listing_not_active", "no longer accepts changes"],
  [409, "command_id_conflict", "already used for a different request"],
  [409, "session_changed", "session and identity are confirmed"],
  [409, "fake_new_rejection", "Auction data is temporarily unavailable"],
  [413, "body_too_large", "Auction data is temporarily unavailable"],
  [415, "unsupported_media_type", "Auction data is temporarily unavailable"],
  [429, "registration_rate_limited", "hourly registration limit"],
  [429, "listing_limit_reached", "limit for active listings"],
  [429, "rate_limited", "Auction data is temporarily unavailable"],
  [503, "consignment_unavailable", "consignment market is temporarily unavailable"],
  [503, "outcome_unknown", "Checking the result of your last submission"],
  [503, "storage_unavailable", "Auction data is temporarily unavailable"],
] as const)("登记响应 %i %s 只显示本地译文", async (http, code, text) => {
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? response(status(true)) : failure(http, code),
  );
  const page = await mountPrivate(NEW);
  try {
    await settle();
    fill(page);
    submitRegistration(page);
    await settle();
    expect(page.text()).toContain(text);
    expect(page.text()).not.toContain("fake server message");
    expect(page.location.pathname).toBe(NEW);
    assertNoSecret(page);
  } finally {
    page.dispose();
    transport.restore();
  }
});
