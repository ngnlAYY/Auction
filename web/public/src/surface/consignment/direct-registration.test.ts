// 免登录非寄售登记：无会话受理、寄售提示登录、结果未知只允许原样重送。

import { expect, test } from "bun:test";

import {
  acceptedReceipt,
  assertNoSecret,
  EQUIPMENT_URL,
  expectedUser,
  fill,
  isWrite,
  memory,
  mountPrivate,
  NEW,
  RETRY,
  receipt,
  requestBody,
  submitRegistration,
} from "./private-fixture.test";
import {
  controlEvent,
  failure,
  fakeClock,
  fakeFetch,
  LISTING_A,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";

const DIRECT_PATH = "/api/v1/consignment/direct-listings";
const DIRECT_COMMANDS = "/api/v1/consignment/direct-commands";
const DIRECT_KEY = "hv.consignment.direct.pending";

type Page = Awaited<ReturnType<typeof mountPrivate>>;

function fillName(page: Page, name: string) {
  controlEvent(
    page.find(
      (node) =>
        node.tag === "input" &&
        node.attributes.type === undefined &&
        node.attributes.inputmode === undefined,
    ),
    "onInput",
    name,
  );
}

function progress(commandId: string) {
  return response({
    result: { http_status: 202, code: "accepted", receipt: receipt(commandId) },
    listing: {
      listing_id: LISTING_A,
      version: 1,
      effective_state: "resolving",
      effective_reason: null,
      expires_at: "2026-11-30T00:00:00Z",
    },
  });
}

test("未登录选寄售只提示登录；非寄售填角色名后不带会话受理并可查进度", async () => {
  const storage = memory();
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return response(status(true));
    if (pathname.startsWith(DIRECT_COMMANDS)) return progress(String(pathname.split("/").at(-1)));
    if (isWrite(request)) return acceptedReceipt(receipt(String(requestBody(request).command_id)));
    return failure(404, "not_found");
  });
  const page = await mountPrivate(NEW, null, storage);
  try {
    await settle();
    expect(page.text()).not.toContain("Sign in with your bidding key first");
    controlEvent(
      page.find((node) => node.tag === "select"),
      "onChange",
      "consigned",
    );
    expect(page.text()).toContain("Consigned listings require signing in.");
    expect(page.button("Submit registration")).toBeNull();

    fill(page, "direct");
    fillName(page, "Fake Seller");
    submitRegistration(page);
    await settle();
    assertNoSecret(page);
    const writes = transport.requests.filter(isWrite);
    expect(writes.map(({ url }) => url.pathname)).toEqual([DIRECT_PATH]);
    expect(expectedUser(writes[0])).toBeNull();
    const body = requestBody(writes[0]);
    expect(Object.keys(body).sort()).toEqual([
      "command_id",
      "equipment_url",
      "price",
      "seller_game_name",
    ]);
    expect(body.seller_game_name).toBe("Fake Seller");
    expect(storage.session.get(DIRECT_KEY)).toBe("null");
    expect(page.text()).toContain("Registration accepted.");

    controlEvent(page.button("Check progress"), "onClick");
    await settle();
    expect(transport.requests.at(-1)?.url.pathname).toBe(`${DIRECT_COMMANDS}/${body.command_id}`);
    expect(page.text()).toContain("Looking up equipment");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("免登录登记超时后锁住命令；刷新查 404 后只能用同一装备原样重送", async () => {
  const clock = fakeClock();
  const storage = memory();
  let writes = 0;
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return response(status(true));
    if (pathname.startsWith(DIRECT_COMMANDS)) return failure(404, "command_not_found");
    if (isWrite(request)) {
      writes += 1;
      if (writes === 1) {
        return new Promise<Response>((_, reject) =>
          request.init?.signal?.addEventListener(
            "abort",
            () => reject(new DOMException("测试超时", "AbortError")),
            { once: true },
          ),
        );
      }
      return acceptedReceipt(receipt(String(requestBody(request).command_id)));
    }
    return failure(404, "not_found");
  });
  let page = await mountPrivate(NEW, null, storage);
  try {
    await settle();
    fill(page, "direct");
    fillName(page, "Fake Seller");
    submitRegistration(page);
    const marker = JSON.parse(storage.session.get(DIRECT_KEY) ?? "null");
    expect(marker).toEqual({
      command_id: requestBody(transport.requests.find(isWrite)).command_id,
      seller_game_name: "Fake Seller",
      world_id: "main",
      eid: "42",
      price: 101,
    });
    await clock.advance(10_000);
    expect(page.text()).toContain("timed out");
    submitRegistration(page);
    expect(transport.requests.filter(isWrite)).toHaveLength(1);

    page.dispose();
    page = await mountPrivate(NEW, null, storage);
    await settle();
    expect(transport.requests.map(({ url }) => url.pathname)).toContain(
      `${DIRECT_COMMANDS}/${marker.command_id}`,
    );
    controlEvent(page.input("password"), "onInput", EQUIPMENT_URL.replace("/42/", "/43/"));
    controlEvent(page.form(RETRY), "onSubmit");
    await settle();
    expect(transport.requests.filter(isWrite)).toHaveLength(1);

    controlEvent(page.input("password"), "onInput", EQUIPMENT_URL);
    controlEvent(page.form(RETRY), "onSubmit");
    await settle();
    const attempts = transport.requests.filter(isWrite);
    expect(attempts).toHaveLength(2);
    expect(attempts[0]?.init?.body === attempts[1]?.init?.body).toBe(true);
    expect(storage.session.get(DIRECT_KEY)).toBe("null");
    expect(page.text()).toContain("Registration accepted.");
    assertNoSecret(page);
  } finally {
    page.dispose();
    transport.restore();
    clock.restore();
  }
});

const OUTCOME_KEY = "hv.consignment.direct.outcome";

test("受理后刷新仍回到进度视图并按原命令 ID 查询；再登记一件后旧回执清除", async () => {
  const storage = memory();
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return response(status(true));
    if (pathname.startsWith(DIRECT_COMMANDS)) return progress(String(pathname.split("/").at(-1)));
    if (isWrite(request)) return acceptedReceipt(receipt(String(requestBody(request).command_id)));
    return failure(404, "not_found");
  });
  let page = await mountPrivate(NEW, null, storage);
  try {
    await settle();
    fill(page, "direct");
    fillName(page, "Fake Seller");
    submitRegistration(page);
    await settle();
    const commandId = requestBody(transport.requests.find(isWrite)).command_id;
    // 不变量 7：结案记录只有命令 ID 与挂单 ID，不含自填名与链接。
    const outcome = JSON.parse(storage.session.get(OUTCOME_KEY) ?? "null");
    expect(Object.keys(outcome).sort()).toEqual(["command_id", "kind", "listing_id"]);
    expect(outcome.command_id).toBe(commandId);
    expect(storage.session.get(OUTCOME_KEY)).not.toContain("Fake Seller");

    page.dispose();
    page = await mountPrivate(NEW, null, storage);
    await settle();
    expect(page.text()).toContain("Registration accepted.");
    controlEvent(page.button("Check progress"), "onClick");
    await settle();
    expect(transport.requests.at(-1)?.url.pathname).toBe(`${DIRECT_COMMANDS}/${commandId}`);
    expect(page.text()).toContain("Looking up equipment");
    expect(transport.requests.filter(isWrite)).toHaveLength(1);

    controlEvent(page.button("Register another item"), "onClick");
    await settle();
    expect(storage.session.get(OUTCOME_KEY)).toBe("null");
    expect(page.text()).not.toContain("Registration accepted.");
    page.dispose();
    page = await mountPrivate(NEW, null, storage);
    await settle();
    expect(page.text()).not.toContain("Registration accepted.");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("首次决定的拒绝刷新后仍可见，开始新登记时清除", async () => {
  const storage = memory();
  let writes = 0;
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return response(status(true));
    if (isWrite(request)) {
      writes += 1;
      return writes === 1
        ? failure(429, "listing_limit_reached")
        : acceptedReceipt(receipt(String(requestBody(request).command_id)));
    }
    return failure(404, "not_found");
  });
  let page = await mountPrivate(NEW, null, storage);
  try {
    await settle();
    fill(page, "direct");
    fillName(page, "Fake Seller");
    submitRegistration(page);
    await settle();
    expect(page.text()).toContain("limit for active listings");
    expect(JSON.parse(storage.session.get(OUTCOME_KEY) ?? "null")).toEqual({
      kind: "rejected",
      status: 429,
      code: "listing_limit_reached",
    });

    page.dispose();
    page = await mountPrivate(NEW, null, storage);
    await settle();
    expect(page.text()).toContain("limit for active listings");
    fill(page, "direct");
    fillName(page, "Fake Seller");
    submitRegistration(page);
    await settle();
    expect(page.text()).not.toContain("limit for active listings");
    expect(page.text()).toContain("Registration accepted.");
    expect(JSON.parse(storage.session.get(OUTCOME_KEY) ?? "null").kind).toBe("accepted");
  } finally {
    page.dispose();
    transport.restore();
  }
});
