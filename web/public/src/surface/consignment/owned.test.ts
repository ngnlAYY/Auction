// 本人列表的有效态、分页与换号后的快照隔离。

import { expect, test } from "bun:test";
import type { ListingView } from "@auction/contracts/dto/public/consignment";

import { buyerOrder } from "./order-fixture.test";
import {
  ALICE,
  BOB,
  expectedUser,
  listing,
  mountPrivate,
  OWNED_PATH,
  pageData,
} from "./private-fixture.test";
import {
  controlEvent,
  DOMText,
  fakeFetch,
  LISTING_A,
  LISTING_B,
  response,
  settle,
} from "./read-fixture.test";

const MINE = "/consignment/me";
const STATES = [
  ["resolving", "Looking up equipment"],
  ["lookup_failed", "Equipment lookup failed"],
  ["ineligible", "Equipment lookup complete. Listing not permitted."],
  ["displayed", "Listed"],
  ["awaiting_deposit", "Awaiting equipment delivery"],
  ["closed", "Closed"],
  ["expired", "Expired"],
] as const satisfies readonly (readonly [ListingView["kind"], string])[];

test("关闭后本人历史仍显示七种视图、各自的标题、安全名称或编号", async () => {
  const entries = STATES.map(([view], index) =>
    listing(view, {
      listing_id: `00000000-0000-4000-8000-00000000001${index}`,
      external_equipment_id: String(42 + index),
    }),
  );
  const transport = fakeFetch(({ url }) => response(pageData(url, entries)));
  const page = await mountPrivate(MINE);
  try {
    await settle();
    expect(page.errors).toEqual([]);
    expect(transport.requests.map(({ url }) => url.pathname)).toEqual([OWNED_PATH]);
    expect(expectedUser(transport.requests[0])).toBe(ALICE.user_id);
    for (const [index, [view, label]] of STATES.entries()) {
      const row = page.find((node) => node.tag === "li" && DOMText(node).includes(label));
      expect(row).not.toBeNull();
      const link = page.find(
        (node) => node.attributes.href === `${MINE}/${entries[index]?.listing_id}`,
      );
      expect(link).not.toBeNull();
      expect(DOMText(link)).toBe(
        ["resolving", "lookup_failed", "expired"].includes(view)
          ? `#${42 + index}`
          : "Fake Oak Staff",
      );
    }
    expect(page.button("Change asking price")).toBeNull();
    expect(page.button("Cancel listing")).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("按世界、模式、有效态筛选，预设与自定义页长只写寄售 URL", async () => {
  const transport = fakeFetch(({ url }) => {
    const world = url.searchParams.get("world");
    const mode = url.searchParams.get("mode");
    const state = url.searchParams.get("state");
    const item = listing(state === "closed" ? "closed" : "resolving", {
      world_id: world === "isekai" ? "isekai" : "main",
      mode: mode === "consigned" ? "consigned" : "direct",
      stored_state: state === "closed" ? "awaiting_deposit" : "resolving",
    });
    return response(pageData(url, [item], 91));
  });
  const page = await mountPrivate(`${MINE}?bid_page=8&proxy_page=6&consignment_page=2`);
  const node = (name: string) => page.find((element) => element.attributes.name === name);
  try {
    await settle();
    expect(transport.requests.at(-1)?.url.searchParams.get("page")).toBe("2");
    for (const [key, value] of [
      ["world", "isekai"],
      ["mode", "consigned"],
      ["state", "closed"],
    ]) {
      controlEvent(node(key ?? ""), "onChange", value);
      await settle();
    }
    expect(Object.fromEntries(transport.requests.at(-1)?.url.searchParams ?? [])).toEqual({
      page: "1",
      page_size: "20",
      world: "isekai",
      mode: "consigned",
      state: "closed",
    });
    expect(page.text()).toContain("The service was disabled");
    expect(
      page.find((element) => element.tag === "option" && element.attributes.value === "rejected"),
    ).not.toBeNull();

    controlEvent(node("page_size"), "onChange", "50");
    await settle();
    const custom = () => page.find((element) => element.attributes.class === "ui-pager__custom");
    controlEvent(node("custom_page_size"), "onInput", "201");
    controlEvent(custom(), "onSubmit");
    await settle();
    expect(transport.requests.at(-1)?.url.searchParams.get("page_size")).toBe("50");
    controlEvent(node("custom_page_size"), "onInput", "37");
    controlEvent(custom(), "onSubmit");
    await settle();
    expect(transport.requests.at(-1)?.url.searchParams.get("page_size")).toBe("37");
    controlEvent(
      page.find(
        (element) => element.tag === "button" && element.attributes["aria-label"] === "Next",
      ),
      "onClick",
    );
    await settle();
    expect(transport.requests.at(-1)?.url.searchParams.get("page")).toBe("2");
    expect(Object.fromEntries(page.location.searchParams)).toEqual({
      bid_page: "8",
      proxy_page: "6",
      consignment_world: "isekai",
      consignment_mode: "consigned",
      consignment_state: "closed",
      consignment_page_size: "37",
      consignment_page: "2",
    });
    expect(page.storage.local.size).toBe(0);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  "consignment_page_size=201",
  "consignment_state=other",
  "consignment_page=1%0A",
  "consignment_world=main&consignment_world=isekai",
])("非法本人书签 %s 不读取，清除后才读取合法空页", async (search) => {
  const transport = fakeFetch(({ url }) => response(pageData(url, [], 0)));
  const page = await mountPrivate(`${MINE}?${search}`);
  try {
    await settle();
    expect(transport.requests).toHaveLength(0);
    const clear = page.find((node) => node.tag === "button" && /Clear/u.test(DOMText(node)));
    controlEvent(clear, "onClick");
    await settle();
    expect(transport.requests).toHaveLength(1);
    expect(page.text()).toContain("No listings");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("本人挂单分页条显示条目范围，没有挂单时显示 0 项", async () => {
  for (const [total, pageNumber, text] of [
    [91, 2, "21–40 / 91 items"],
    [0, 1, "0–0 / 0 items"],
  ] as const) {
    const transport = fakeFetch(({ url }) =>
      response(pageData(url, total === 0 ? [] : [listing("resolving")], total)),
    );
    const page = await mountPrivate(`${MINE}?consignment_page=${pageNumber}`);
    try {
      await settle();
      const range = page.find((node) => node.attributes.class === "ui-pager__range");
      expect(DOMText(range)).toBe(text);
    } finally {
      page.dispose();
      transport.restore();
    }
  }
});

test("本人订单分页条显示条目范围，没有订单时显示 0 项", async () => {
  for (const [total, items, text] of [
    [1, [buyerOrder()], "1–1 / 1 items"],
    [0, [], "0–0 / 0 items"],
  ] as const) {
    const transport = fakeFetch(({ url }) =>
      response({
        items,
        total,
        page: Number(url.searchParams.get("page")),
        page_size: Number(url.searchParams.get("page_size")),
      }),
    );
    const page = await mountPrivate("/consignment/orders");
    try {
      await settle();
      const range = page.find((node) => node.attributes.class === "ui-pager__range");
      expect(DOMText(range)).toBe(text);
    } finally {
      page.dispose();
      transport.restore();
    }
  }
});

test("换号立即撤下旧私有行、取消重读并丢弃迟到的旧列表", async () => {
  const late = Promise.withResolvers<Response>();
  const bob = Promise.withResolvers<Response>();
  let aliceReads = 0;
  const transport = fakeFetch((request) => {
    if (expectedUser(request) === BOB.user_id) return bob.promise;
    aliceReads += 1;
    return aliceReads === 1
      ? response(pageData(request.url, [listing("displayed")]))
      : late.promise;
  });
  const page = await mountPrivate(MINE);
  try {
    await settle();
    expect(page.text()).toContain("Fake Oak Staff");
    page.focus();
    await settle();
    const outstanding = transport.requests.at(-1);
    expect(outstanding?.init?.signal?.aborted).toBe(false);
    await page.identify(BOB);
    expect(outstanding?.init?.signal?.aborted).toBe(true);
    expect(page.text()).not.toContain("Fake Oak Staff");
    expect(expectedUser(transport.requests.at(-1))).toBe(BOB.user_id);
    const aliceUrl = new URL(`https://auction.test${OWNED_PATH}?page=1&page_size=20`);
    late.resolve(response(pageData(aliceUrl, [listing("displayed", { listing_id: LISTING_A })])));
    bob.resolve(
      response(
        pageData(aliceUrl, [
          listing("lookup_failed", { listing_id: LISTING_B, external_equipment_id: "99" }),
        ]),
      ),
    );
    await settle();
    expect(page.text()).toContain("#99");
    expect(page.text()).not.toContain("Fake Oak Staff");
    expect(page.find((node) => node.attributes.href === `${MINE}/${LISTING_A}`)).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});
