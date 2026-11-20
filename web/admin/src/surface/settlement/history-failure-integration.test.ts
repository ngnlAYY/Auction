// 用内存历史和真实结算页检验目录归并与旧响应隔离。

import { expect, spyOn, test } from "bun:test";
import { adminContract } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { settle } from "../solid-dom-fixture";
import { requireElement, withAdminTree } from "./settlement-tree-fixture.test";

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const OLD_AUCTION_ID = "00000000-0000-4000-8000-000000000020";
const MAIN = "/settlement?world=main&page_size=10";
const ISEKAI = "/settlement?world=isekai&page_size=10";
const CATALOG = "/api/v1/admin/settlement-catalog";
const REPORT = `/api/v1/admin/auctions/${AUCTION_ID}/settlement-report`;
const RECOVERY = `/api/v1/admin/auctions/${AUCTION_ID}/recovery-report?include=send_attribution`;

type SettlementTree = Parameters<Parameters<typeof withAdminTree>[1]>[0];

function loadPage() {
  return loadRenderModule<typeof import("./page")>(`${import.meta.dir}/page.tsx`, "page");
}

function catalogReply(
  world: "main" | "isekai",
  page: number,
  total: number,
  auctionId: string | null,
) {
  const auctions =
    auctionId === null
      ? []
      : [
          {
            auction_id: auctionId,
            world_id: world,
            sequence: 5,
            settled_at: "2025-11-16T09:46:40.110000Z",
          },
        ];
  return {
    body: JSON.stringify({
      code: "ok",
      message: "结算目录已加载",
      data: { auctions, pagination: { page, page_size: 10, total } },
    }),
  };
}

function detailReplies(world: "main" | "isekai") {
  const report = adminContract("settlement-report").replace(
    '"world_id": "main"',
    `"world_id": "${world}"`,
  );
  return [
    { body: report },
    {
      body: JSON.stringify({
        code: "ok",
        message: "",
        data: { auction_id: AUCTION_ID, recoveries: [] },
      }),
    },
  ];
}

function select(tree: SettlementTree, id: "settlement-world" | "settlement-period") {
  return requireElement(
    tree.find((node) => node.getAttribute("id") === id),
    id,
  );
}

async function expectReport(tree: SettlementTree) {
  for (let round = 0; round < 4 && tree.find((node) => node.tag === "tbody") === null; round += 1) {
    await settle();
  }
  expect(select(tree, "settlement-period").value).toBe(AUCTION_ID);
  expect(tree.find((node) => node.tag === "td" && node.textContent === "1h 00m")).not.toBeNull();
}

test("拒绝 pushState 后仍选中异世界的默认场次", async () => {
  const { page } = await loadPage();
  await withAdminTree(MAIN, async (tree) => {
    const requests = tree.requests.installFetch([
      catalogReply("main", 1, 0, null),
      catalogReply("isekai", 1, 1, AUCTION_ID),
      ...detailReplies("isekai"),
    ]);
    tree.mount(page);
    await settle();
    expect(select(tree, "settlement-world").value).toBe("main");
    const originalAddress = tree.fake.location.href;
    const writes = [...tree.fake.writes];
    // 模拟浏览器持续拒绝历史写入，包括目录完成后的地址规范化。
    tree.fake.rejectWrites(true);
    const push = spyOn(tree.fake.history, "pushState");
    try {
      select(tree, "settlement-world").change("isekai");
      expect(push).toHaveBeenCalledWith(
        expect.anything(),
        "",
        "/settlement?page_size=10&world=isekai",
      );
    } finally {
      push.mockRestore();
    }
    await settle();

    expect(select(tree, "settlement-world").value).toBe("isekai");
    await expectReport(tree);
    expect(tree.fake.location.href).toBe(originalAddress);
    expect(tree.fake.writes).toEqual(writes);
    expect(requests.map((request) => request.url)).toEqual([
      `${CATALOG}?page=1&page_size=10&world_id=main`,
      `${CATALOG}?page=1&page_size=10&world_id=isekai`,
      REPORT,
      RECOVERY,
    ]);
  });
});

test("空尾页钳制的 replaceState 被拒绝后仍采用末页目录", async () => {
  const { page } = await loadPage();
  await withAdminTree(`${MAIN}&page=3`, async (tree) => {
    const requests = tree.requests.installFetch([
      catalogReply("main", 3, 11, null),
      catalogReply("main", 2, 11, AUCTION_ID),
      ...detailReplies("main"),
    ]);
    const originalAddress = tree.fake.location.href;
    const writes = [...tree.fake.writes];
    tree.fake.rejectWrites(true);
    const replace = spyOn(tree.fake.history, "replaceState");
    try {
      tree.mount(page);
      await settle();
      await expectReport(tree);
      expect(replace).toHaveBeenCalledWith(
        expect.anything(),
        "",
        "/settlement?page_size=10&world=main&page=2",
      );
    } finally {
      replace.mockRestore();
    }
    expect(tree.fake.location.href).toBe(originalAddress);
    expect(tree.fake.writes).toEqual(writes);
    expect(requests.map((request) => request.url)).toEqual([
      `${CATALOG}?page=3&page_size=10&world_id=main`,
      `${CATALOG}?page=2&page_size=10&world_id=main`,
      REPORT,
      RECOVERY,
    ]);
  });
});

test("后退和前进后迟到的目录响应不能选中旧场次或覆盖地址", async () => {
  const { page } = await loadPage();
  await withAdminTree(MAIN, async (tree) => {
    tree.fake.history.pushState(null, "", ISEKAI);
    const requests = tree.requests.installFetch([
      catalogReply("isekai", 1, 1, AUCTION_ID),
      ...detailReplies("isekai"),
    ]);
    const oldIsekai = Promise.withResolvers<Response>();
    const oldMain = Promise.withResolvers<Response>();
    const queuedFetch = globalThis.fetch;
    const pending = [oldIsekai.promise, oldMain.promise];
    const fetch = spyOn(globalThis, "fetch").mockImplementation(
      Object.assign(
        (...args: Parameters<typeof queuedFetch>) => pending.shift() ?? queuedFetch(...args),
        { preconnect: queuedFetch.preconnect },
      ),
    );
    try {
      tree.mount(page);
      await settle();
      expect(fetch).toHaveBeenCalledTimes(1);
      tree.fake.history.back();
      await settle();
      expect(fetch).toHaveBeenCalledTimes(2);

      oldIsekai.resolve(new Response(catalogReply("isekai", 1, 1, OLD_AUCTION_ID).body));
      await settle();
      expect(select(tree, "settlement-world").value).toBe("main");
      expect(select(tree, "settlement-period").value).toBe("");
      expect(tree.fake.location.search).toBe("?world=main&page_size=10");

      tree.fake.history.forward();
      oldMain.resolve(new Response(catalogReply("main", 1, 1, OLD_AUCTION_ID).body));
      await settle();
      expect(select(tree, "settlement-world").value).toBe("isekai");
      await expectReport(tree);
      expect(tree.fake.location.searchParams.get("world")).toBe("isekai");
      expect(tree.fake.location.searchParams.get("auction")).toBe(AUCTION_ID);
      expect(requests.map((request) => request.url)).toEqual([
        `${CATALOG}?page=1&page_size=10&world_id=isekai`,
        REPORT,
        RECOVERY,
      ]);
    } finally {
      fetch.mockRestore();
    }
  });
});
