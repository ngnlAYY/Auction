// 公开目录的能力门、筛选、分页、列表卡片和错误状态测试。

import { expect, test } from "bun:test";
import { createComponent } from "solid-js";
import { LISTINGS_PATH, pageResponse, summary } from "./order-fixture.test";
import {
  controlEvent,
  DOMText,
  failure,
  fakeClock,
  fakeFetch,
  LISTING_A,
  LISTING_B,
  mountConsignment,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";
import { loadCatalog } from "./render-fixture.test";

test.each([
  ["关闭", () => response(status(false)), "The consignment market is disabled", 0, false],
  ["开启可登记", () => response(status(true)), "Fake Oak Staff", 1, true],
  ["开启暂不可登记", () => response(status(true, false)), "Fake Oak Staff", 1, false],
  [
    "查询失败",
    () => failure(503, "consignment_unavailable"),
    "The consignment market is temporarily unavailable",
    0,
    false,
  ],
] as const)(
  "%s时按能力渲染并决定是否读取目录",
  async (_, statusReply, text, count, registerShown) => {
    const { ConsignmentCatalog } = await loadCatalog();
    const pending = Promise.withResolvers<Response>();
    let retry = false;
    const transport = fakeFetch(({ url }) =>
      url.pathname === STATUS_PATH
        ? retry
          ? response(status(true))
          : pending.promise
        : pageResponse(url, [summary()]),
    );
    const page = await mountConsignment(
      "/consignment",
      () => createComponent(ConsignmentCatalog, {}),
      true,
    );
    try {
      await settle();
      expect(page.text()).toContain("Loading");
      expect(transport.requests.map(({ url }) => url.pathname)).toEqual([STATUS_PATH]);
      pending.resolve(statusReply());
      await settle();
      expect(page.text()).toContain(text);
      expect(transport.requests.filter(({ url }) => url.pathname === LISTINGS_PATH)).toHaveLength(
        count,
      );
      // 登记入口仅在登记就绪时出现在工具栏；我的挂单与我的订单在账号弹出框，页面本身始终没有。
      const register = page.find((node) => node.attributes.href === "/consignment/new");
      expect(register !== null).toBe(registerShown);
      for (const href of ["/consignment/me", "/consignment/orders"]) {
        expect(page.find((node) => node.attributes.href === href)).toBeNull();
      }
      if (text.includes("temporarily unavailable")) {
        retry = true;
        controlEvent(
          page.find((node) => node.tag === "button" && DOMText(node) === "Retry"),
          "onClick",
        );
        await settle();
        expect(page.text()).toContain("Fake Oak Staff");
      }
    } finally {
      page.dispose();
      transport.restore();
    }
  },
);

test("筛选由地址恢复、写回和后退，世界归外壳键，保留拍卖查询与片段", async () => {
  const { ConsignmentCatalog } = await loadCatalog();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? response(status(true)) : pageResponse(url, []),
  );
  const page = await mountConsignment(
    "/consignment?world=isekai&mode=consigned&type=sta,shd&name=a%25_%E5%89%91&consignment_page=2&bid_page=5&proxy_page=4&page_size=50#kept",
    () => createComponent(ConsignmentCatalog, {}),
  );
  const button = (text: string) =>
    page.find((node) => node.tag === "button" && DOMText(node) === text);
  const input = () => page.find((node) => node.attributes.name === "name");
  try {
    await settle();
    expect(transport.requests[1]?.url.search).toBe(
      "?world=isekai&mode=consigned&category=Staff%2CShield&q=a%25_%E5%89%91&page=2&page_size=20",
    );
    expect(input()?.attributes.value).toBe("a%_剑");
    expect(page.find((node) => node.attributes.href === "/consignment/me")).toBeNull();

    controlEvent(button("Direct sale"), "onClick");
    await settle();
    // 点击即生效并回到第一页；多选分类按规范顺序以逗号连接。
    expect(transport.requests.at(-1)?.url.search).toBe(
      "?world=isekai&mode=direct&category=Staff%2CShield&q=a%25_%E5%89%91&page=1&page_size=20",
    );
    controlEvent(input(), "onInput", "oak + %");
    expect(transport.requests.filter(({ url }) => url.pathname === LISTINGS_PATH)).toHaveLength(2);
    controlEvent(
      page.find((node) => node.attributes.class === "public-consignment-search"),
      "onSubmit",
    );
    await settle();
    expect(transport.requests.at(-1)?.url.search).toBe(
      "?world=isekai&mode=direct&category=Staff%2CShield&q=oak+%2B+%25&page=1&page_size=20",
    );
    expect(Object.fromEntries(page.location.searchParams)).toEqual({
      world: "isekai",
      mode: "direct",
      type: "sta,shd",
      name: "oak + %",
      bid_page: "5",
      proxy_page: "4",
      page_size: "50",
    });
    expect(page.location.hash).toBe("#kept");
    page.back();
    await settle();
    expect(input()?.attributes.value).toBe("a%_剑");
    expect(transport.requests.at(-1)?.url.search).toContain("mode=direct");
  } finally {
    page.dispose();
    transport.restore();
  }
});

const FAKE_EQUIPMENT_LINK = "HTTPS://example.invalid/EqUiP/42?KeY=0000000042";
const INVALID_LINK_NOTICE = "A keyword contains invalid characters";

test.each(["name"] as const)("粘贴装备链接提交 %s 时拒绝写历史和读取目录", async (field) => {
  const { ConsignmentCatalog } = await loadCatalog();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? response(status(true)) : pageResponse(url, []),
  );
  const page = await mountConsignment("/consignment?world=main&consignment_page=2", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  try {
    await settle();
    expect(transport.requests.at(-1)?.url.search).toBe("?world=main&page=2&page_size=20");
    const address = page.location.href;
    const writes = page.historyWrites.length;
    const reads = () => transport.requests.filter(({ url }) => url.pathname === LISTINGS_PATH);
    const readCount = reads().length;
    controlEvent(
      page.find((node) => node.attributes.name === field),
      "onInput",
      FAKE_EQUIPMENT_LINK,
    );
    controlEvent(
      page.find((node) => node.attributes.class === "public-consignment-search"),
      "onSubmit",
    );
    await settle();
    page.focus();
    await settle();
    expect(page.location.href === address).toBe(true);
    expect(page.historyWrites).toHaveLength(writes);
    expect(reads()).toHaveLength(readCount);
    expect(transport.requests.some(({ url }) => url.searchParams.has(field))).toBe(false);
    expect(page.text()).toContain(INVALID_LINK_NOTICE);
    const clear = page.find((node) => node.tag === "button" && DOMText(node) === "Clear");
    controlEvent(clear, "onClick");
    await settle();
    expect(transport.requests.at(-1)?.url.search).toBe("?world=main&page=1&page_size=20");
    expect(page.location.search).toBe("?world=main");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each(["name"] as const)(
  "恢复含装备链接的 %s 书签时清除地址中的凭证且不读取或回显",
  async (field) => {
    const { ConsignmentCatalog } = await loadCatalog();
    const transport = fakeFetch(({ url }) =>
      url.pathname === STATUS_PATH ? response(status(true)) : pageResponse(url, []),
    );
    const key = field;
    const params = new URLSearchParams({ world: "main", [key]: FAKE_EQUIPMENT_LINK });
    const page = await mountConsignment(`/consignment?${params}#kept`, () =>
      createComponent(ConsignmentCatalog, {}),
    );
    try {
      await settle();
      expect(page.location.searchParams.has(key)).toBe(false);
      expect(page.location.search).toBe("?world=main");
      expect(page.location.hash).toBe("#kept");
      expect(transport.requests.map(({ url }) => url.pathname)).toEqual([STATUS_PATH]);
      expect(transport.requests.some(({ url }) => url.searchParams.has(field))).toBe(false);
      expect(page.find((node) => node.attributes.name === field)?.attributes.value === "").toBe(
        true,
      );
      expect(page.text().includes(FAKE_EQUIPMENT_LINK)).toBe(false);
      expect(page.text()).toContain(INVALID_LINK_NOTICE);
      controlEvent(
        page.find((node) => node.tag === "button" && DOMText(node) === "Clear"),
        "onClick",
      );
      await settle();
      expect(transport.requests.at(-1)?.url.search).toBe("?world=main&page=1&page_size=20");
    } finally {
      page.dispose();
      transport.restore();
    }
  },
);

test("切换筛选和离开目录均中止传给 fetch 的在途请求", async () => {
  const { ConsignmentCatalog } = await loadCatalog();
  const pending = Promise.withResolvers<void>();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH
      ? response(status(true))
      : pending.promise.then(() => pageResponse(url, [])),
  );
  const page = await mountConsignment("/consignment", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  try {
    await settle();
    const reads = () => transport.requests.filter(({ url }) => url.pathname === LISTINGS_PATH);
    expect(reads()).toHaveLength(1);
    const first = reads()[0]?.init?.signal;
    expect(first?.aborted).toBe(false);
    controlEvent(
      page.find((node) => node.attributes.name === "name"),
      "onInput",
      "oak",
    );
    controlEvent(
      page.find((node) => node.attributes.class === "public-consignment-search"),
      "onSubmit",
    );
    await settle();
    const abortedOnFilterChange = first?.aborted;
    expect(reads()).toHaveLength(2);
    const next = reads()[1];
    expect(next?.url.searchParams.get("q")).toBe("oak");
    expect(next?.init?.signal?.aborted).toBe(false);
    page.dispose();
    expect({ abortedOnFilterChange, abortedOnLeave: next?.init?.signal?.aborted }).toEqual({
      abortedOnFilterChange: true,
      abortedOnLeave: true,
    });
  } finally {
    page.dispose();
    pending.resolve();
    await settle();
    transport.restore();
  }
});

test("预设、自定义 37、翻页与非法页长只操作寄售分页", async () => {
  const { ConsignmentCatalog } = await loadCatalog();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? response(status(true)) : pageResponse(url, [summary()], 117),
  );
  const page = await mountConsignment("/consignment?bid_page=8&proxy_page=6", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  const input = (name: string) => page.find((node) => node.attributes.name === name);
  const custom = () => page.find((node) => node.attributes.class === "ui-pager__custom");
  try {
    await settle();
    for (const size of [10, 20, 50, 100]) {
      expect(
        page.find((node) => node.tag === "option" && node.attributes.value === String(size)),
      ).not.toBeNull();
    }
    controlEvent(input("page_size"), "onChange", "50");
    await settle();
    expect(page.location.searchParams.get("consignment_page_size")).toBe("50");
    expect(transport.requests.at(-1)?.url.search).toBe("?page=1&page_size=50");

    controlEvent(input("custom_page_size"), "onInput", "201");
    controlEvent(custom(), "onSubmit");
    await settle();
    expect(transport.requests).toHaveLength(3);
    expect(input("custom_page_size")?.attributes["aria-invalid"]).toBe("true");

    controlEvent(input("custom_page_size"), "onInput", "37");
    controlEvent(custom(), "onSubmit");
    await settle();
    expect(transport.requests.at(-1)?.url.search).toBe("?page=1&page_size=37");
    expect(page.location.searchParams.get("consignment_page_size")).toBe("37");
    expect(input("custom_page_size")?.attributes.value).toBe("37");

    controlEvent(
      page.find((node) => node.tag === "button" && node.attributes["aria-label"] === "Next"),
      "onClick",
    );
    await settle();
    expect(page.location.searchParams.get("consignment_page")).toBe("2");
    expect(transport.requests.at(-1)?.url.search).toBe("?page=2&page_size=37");
    expect(page.location.searchParams.get("bid_page")).toBe("8");
    expect(page.location.searchParams.get("proxy_page")).toBe("6");
    expect(page.location.searchParams.has("page_size")).toBe(false);
    expect(page.location.searchParams.has("movement_page_size")).toBe(false);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("非寄售与待寄卡片使用不同告知且待寄没有交易控件", async () => {
  const { ConsignmentCatalog } = await loadCatalog();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH
      ? response(status(true))
      : pageResponse(url, [summary(), summary(LISTING_B)]),
  );
  const page = await mountConsignment("/consignment", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  try {
    await settle();
    const direct = page.find(
      (node) => node.tag === "article" && DOMText(node).includes("Fake Oak Staff"),
    );
    const deposit = page.find(
      (node) => node.tag === "article" && DOMText(node).includes("Fake Iron Shield"),
    );
    expect(DOMText(direct)).toContain("1,234c");
    expect(DOMText(direct)).toContain("Fake Seller");
    expect(DOMText(direct)).toContain("Contact the seller in game");
    expect(DOMText(direct)).not.toContain("does not yet hold");
    expect(DOMText(deposit)).toContain("does not yet hold the equipment");
    expect(DOMText(deposit)).not.toContain("Contact the seller");
    const href = (listingId: string) => `/consignment/listings/${listingId}`;
    expect(page.find((node) => node.attributes.href === href(LISTING_A))).not.toBeNull();
    expect(page.find((node) => node.attributes.href === href(LISTING_B))).not.toBeNull();
    expect(
      page.find(
        (node) =>
          node.tag === "article" &&
          DOMText(node).includes("Fake Iron Shield") &&
          node.nodes.some((region) => findTransactionControl(region(), href(LISTING_B))),
      ),
    ).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

function findTransactionControl(region: unknown, detailsHref: string): boolean {
  if (Array.isArray(region))
    return region.some((node) => findTransactionControl(node, detailsHref));
  if (typeof region === "function") return findTransactionControl(region(), detailsHref);
  if (region === null || typeof region !== "object" || !("tag" in region)) return false;
  if (region.tag === "button" || region.tag === "form") return true;
  if (
    region.tag === "a" &&
    "attributes" in region &&
    region.attributes !== null &&
    typeof region.attributes === "object" &&
    "href" in region.attributes
  ) {
    return region.attributes.href !== detailsHref;
  }
  return (
    "nodes" in region &&
    Array.isArray(region.nodes) &&
    region.nodes.some((node) => findTransactionControl(node, detailsHref))
  );
}

test("服务返回合法空页时保留页码和空态", async () => {
  const { ConsignmentCatalog } = await loadCatalog();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? response(status(true)) : pageResponse(url, [], 1),
  );
  const page = await mountConsignment("/consignment?consignment_page=3", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  try {
    await settle();
    expect(page.text()).toContain("No listings match the selected filters");
    expect(page.location.searchParams.get("consignment_page")).toBe("3");
    expect(transport.requests).toHaveLength(2);
    expect(page.find((node) => node.tag === "article")).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["invalid_query", () => failure(400, "invalid_query"), "Try widening your filters"],
  [
    "503",
    () => failure(503, "consignment_unavailable"),
    "The consignment market is temporarily unavailable",
  ],
  [
    "transport",
    () => {
      throw new Error("fake network failure");
    },
    "Network connection failed",
  ],
] as const)("目录错误 %s 安全展示且响应恢复后可继续读取", async (_, reply, message) => {
  const { ConsignmentCatalog } = await loadCatalog();
  let resolved = false;
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === STATUS_PATH) return response(status(true));
    return resolved ? pageResponse(url, [summary()]) : reply();
  });
  const page = await mountConsignment("/consignment", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  try {
    await settle();
    expect(page.text()).toContain(message);
    expect(page.text()).not.toContain("fake server message");
    expect(page.text()).not.toContain("fake network failure");
    expect(page.text()).not.toContain("No listings match");
    resolved = true;
    const button = page.find(
      (node) => node.tag === "button" && ["Retry", "Clear"].includes(DOMText(node)),
    );
    controlEvent(button, "onClick");
    await settle();
    expect(page.text()).toContain("Fake Oak Staff");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("可见目录重读后撤下旧卡片，退出取消可见轮询", async () => {
  const { ConsignmentCatalog } = await loadCatalog();
  let reads = 0;
  const transport = fakeFetch(({ url }) => {
    if (url.pathname === STATUS_PATH) return response(status(true));
    reads += 1;
    return pageResponse(url, reads === 1 ? [summary()] : []);
  });
  const clock = fakeClock();
  const page = await mountConsignment("/consignment", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  try {
    await settle();
    expect(page.text()).toContain("Fake Oak Staff");
    await clock.advance(30_000);
    expect(page.text()).not.toContain("Fake Oak Staff");
    expect(page.text()).toContain("No listings match the selected filters");
    expect(reads).toBe(2);
    page.focus();
    await settle();
    expect(reads).toBe(3);
    expect(clock.activeIn(30_000)).toBe(1);
    page.dispose();
    expect(clock.activeIn(30_000)).toBe(0);
    await clock.advance(30_000);
    expect(reads).toBe(3);
  } finally {
    page.dispose();
    clock.restore();
    transport.restore();
  }
});

test("非法书签不读取目录，清除筛选后才发起默认查询", async () => {
  const { ConsignmentCatalog } = await loadCatalog();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? response(status(true)) : pageResponse(url, [summary()]),
  );
  const page = await mountConsignment("/consignment?mode=direct&mode=consigned&page_size=50", () =>
    createComponent(ConsignmentCatalog, {}),
  );
  try {
    await settle();
    expect(transport.requests.map(({ url }) => url.pathname)).toEqual([STATUS_PATH]);
    expect(page.text()).toContain("Try widening your filters");
    controlEvent(
      page.find((node) => node.tag === "button" && DOMText(node) === "Clear"),
      "onClick",
    );
    await settle();
    expect(transport.requests[1]?.url.search).toBe("?page=1&page_size=20");
    expect(page.location.search).toBe("?page_size=50");
    expect(page.text()).toContain("Fake Oak Staff");
  } finally {
    page.dispose();
    transport.restore();
  }
});
