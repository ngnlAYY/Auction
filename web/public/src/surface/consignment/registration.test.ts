// 登记表单的能力、验证、报价、私密输入和提交接线。

import { expect, test } from "bun:test";

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
} from "./private-fixture.test";
import {
  failure,
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
  ["关闭", () => response(status(false)), "service is disabled"],
  ["只读", () => response(status(true, false)), "service is temporarily unavailable"],
  ["失败", () => failure(503, "consignment_unavailable"), "service is temporarily unavailable"],
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
  ["direct", EQUIPMENT_URL, 0],
  ["direct", ISEKAI_URL, 0],
  ["consigned", EQUIPMENT_URL, 500],
  ["consigned", ISEKAI_URL, 500],
] as const)("%s 四种组合之一的登记 202 立即进入本人详情", async (mode, url, fee) => {
  const lookup = Promise.withResolvers<Response>();
  const transport = fakeFetch((request) => {
    if (request.url.pathname === STATUS_PATH) return response(status(true));
    if (request.url.pathname === LISTINGS_PATH) {
      const body = requestBody(request);
      expect(typeof body.command_id).toBe("string");
      expect(body.equipment_url === url).toBe(true);
      expect([body.mode, body.price, body.expected_fee_bps]).toEqual([mode, 101, fee]);
      return acceptedReceipt(receipt(String(body.command_id)));
    }
    return lookup.promise;
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
    expect(transport.requests.at(-1)?.url.pathname).toBe(`${OWNED_PATH}/${LISTING_A}`);
    expect(expectedUser(transport.requests.find(isWrite))).toBe(ALICE.user_id);
    expect(page.text()).toContain("Loading");
    assertNoSecret(page);
    lookup.resolve(response(listing("resolving")));
    await settle();
    expect(page.text()).toContain("Looking up equipment");
  } finally {
    page.dispose();
    transport.restore();
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
  [404, "consignment_disabled", "service is disabled"],
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
  [503, "consignment_unavailable", "service is temporarily unavailable"],
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
