// 用内存历史和真实结算页检验目录归并与旧响应隔离。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { installFixtureFetch, recordRequest } from "../../api/api-request-fixture";
import { adminContract } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import * as pageSizePreference from "../pagination/preferences";
import type { AdminActionFeedback } from "../presentation";
import { createComponent, settle } from "../solid-dom-fixture";
import { currentTestDocument } from "../test-dom-fixture";
import type { RecoveryConfirmationDraft } from "./recovery";
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
    const warn = spyOn(console, "warn");
    try {
      tree.mount(page);
      await settle();
      await expectReport(tree);
      expect(
        warn.mock.calls.filter(([message]) => String(message).includes("STRICT_READ_UNTRACKED")),
      ).toEqual([]);
      expect(replace).toHaveBeenCalledWith(
        expect.anything(),
        "",
        "/settlement?page_size=10&world=main&page=2",
      );
    } finally {
      warn.mockRestore();
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

const FAILURE = {
  status: 503,
  body: '{"code":"temporarily_unavailable","message":"暂不可用","data":null,"request_id":null}',
};
const MAIN_CATALOG = `${CATALOG}?page=1&page_size=10&world_id=main`;
const LOOKUP = `${MAIN_CATALOG}&sequence=5`;
const RECOVERY_BODY = adminContract("recovery-report")
  .replaceAll("00000000-0000-4000-8000-000000000002", AUCTION_ID)
  .replace('"allowed_commands": []', '"allowed_commands": ["reclaimed"]');

function byId(tree: SettlementTree, id: string) {
  return requireElement(
    tree.find((node) => node.id === id),
    id,
  );
}

function serviceFailure() {
  return new Response(FAILURE.body, { status: FAILURE.status });
}

for (const [read, path, heading] of [
  ["catalog", MAIN_CATALOG, "settlement-selector-title"],
  ["period", LOOKUP, "settlement-selector-title"],
  ["report", REPORT, "settlement-report-panel-title"],
  ["recovery", RECOVERY, "settlement-recovery-title"],
] as const) {
  test(`读取 ${read} 失败后只重试同一个 GET，成功后将焦点交给对应标题`, async () => {
    const { page } = await loadPage();
    const href = read === "period" ? `${MAIN}&sequence=5` : MAIN;
    await withAdminTree(href, async (tree) => {
      const pending = Promise.withResolvers<Response>();
      const requests: ReturnType<typeof recordRequest>[] = [];
      const responseBodies = new Map([
        [MAIN_CATALOG, catalogReply("main", 1, 1, read === "period" ? null : AUCTION_ID).body],
        [LOOKUP, catalogReply("main", 1, 1, AUCTION_ID).body],
        [REPORT, adminContract("settlement-report")],
        [RECOVERY, RECOVERY_BODY],
      ]);
      let attempts = 0;
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        requests.push(request);
        const body = responseBodies.get(request.url);
        if (body === undefined) throw new Error(`未预期的请求 ${request.url}`);
        if (request.url === path) {
          attempts += 1;
          if (attempts === 1) return serviceFailure();
          if (attempts === 2) return pending.promise;
        }
        return new Response(body);
      });
      try {
        tree.mount(page);
        await settle();
        const id = `settlement-${read}-retry`;
        const retry = byId(tree, id);
        const request = { url: path, method: "GET", body: "" };
        expect(requests.filter(({ url }) => url === path)).toEqual([request]);
        const before = requests.length;
        retry.focus();
        retry.click();
        retry.dispatchClick();
        await settle();
        expect(byId(tree, id)).toBe(retry);
        expect(retry.getAttribute("aria-disabled")).toBe("true");
        expect(currentTestDocument()?.activeElement).toBe(retry);
        expect(requests.slice(before)).toEqual([request]);

        pending.resolve(serviceFailure());
        await settle();
        expect(byId(tree, id)).toBe(retry);
        expect(retry.getAttribute("aria-disabled")).toBeNull();
        retry.click();
        await settle();
        expect(requests.filter(({ url }) => url === path)).toEqual([request, request, request]);
        expect(tree.find((node) => node.id === id)).toBeNull();
        expect(currentTestDocument()?.activeElement).toBe(byId(tree, heading));
        await expectReport(tree);
        expect(
          tree.find((node) => node.tag === "button" && node.textContent === "确认已回收"),
        ).not.toBeNull();
      } finally {
        restore();
      }
    });
  });
}

test("两份报告同时失败时独立重读，报告身份错误也可按原地址重试", async () => {
  const { page } = await loadPage();
  await withAdminTree(MAIN, async (tree) => {
    const report = adminContract("settlement-report");
    const requests = tree.requests.installFetch([
      catalogReply("main", 1, 1, AUCTION_ID),
      { body: report.replace('"world_id": "main"', '"world_id": "isekai"') },
      FAILURE,
      { body: report },
      { body: RECOVERY_BODY },
    ]);
    tree.mount(page);
    await settle();
    const retryReport = byId(tree, "settlement-report-retry");
    const retryRecovery = byId(tree, "settlement-recovery-retry");
    expect(tree.find((node) => node.tag === "tbody")).toBeNull();
    expect(
      tree.find((node) => node.tag === "button" && node.textContent === "确认已回收"),
    ).toBeNull();
    retryReport.focus();
    retryReport.click();
    retryReport.dispatchClick();
    select(tree, "settlement-world").focus();
    await settle();
    expect(currentTestDocument()?.activeElement).toBe(select(tree, "settlement-world"));
    expect(byId(tree, "settlement-recovery-retry")).toBe(retryRecovery);
    expect(requests.slice(3).map(({ url }) => url)).toEqual([REPORT]);
    await expectReport(tree);
    retryRecovery.click();
    await settle();
    expect(requests.slice(3).map(({ url }) => url)).toEqual([REPORT, RECOVERY]);
    expect(
      tree.find((node) => node.tag === "button" && node.textContent === "确认已回收"),
    ).not.toBeNull();
  });
});

test("同页重选恢复默认目录，带参重进补查，矛盾链接保留地址且没有重试", async () => {
  const { page } = await loadPage();
  await withAdminTree(ISEKAI, async (tree) => {
    const requests = tree.requests.installFetch([
      catalogReply("isekai", 1, 0, null),
      catalogReply("main", 1, 1, AUCTION_ID),
      ...detailReplies("main"),
      catalogReply("main", 1, 1, OLD_AUCTION_ID),
      ...detailReplies("main"),
      catalogReply("main", 1, 1, AUCTION_ID),
    ]);
    tree.mount(page);
    await settle();
    expect(select(tree, "settlement-world").value).toBe("isekai");
    tree.router.push("/settlement");
    await settle();
    expect(select(tree, "settlement-world").value).toBe("all");
    await expectReport(tree);

    const deep = `${MAIN}&auction=${AUCTION_ID}&sequence=5`;
    tree.router.replace(deep);
    await settle();
    expect(select(tree, "settlement-world").value).toBe("main");
    expect(requests.map(({ url }) => url)).toEqual([
      `${CATALOG}?page=1&page_size=10&world_id=isekai`,
      `${CATALOG}?page=1&page_size=10`,
      REPORT,
      RECOVERY,
      MAIN_CATALOG,
      REPORT,
      RECOVERY,
      LOOKUP,
    ]);
    expect(select(tree, "settlement-period").value).toBe(AUCTION_ID);
    const conflict = `${MAIN}&auction=${OLD_AUCTION_ID}&sequence=6`;
    const conflictRequests: ReturnType<typeof recordRequest>[] = [];
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      conflictRequests.push(request);
      return request.url === MAIN_CATALOG
        ? new Response(catalogReply("main", 1, 1, OLD_AUCTION_ID).body)
        : serviceFailure();
    });
    try {
      tree.router.push(conflict);
      await settle();
    } finally {
      restore();
    }
    expect(conflictRequests.some(({ url }) => url.includes("sequence=6"))).toBe(false);
    expect(tree.fake.location.pathname + tree.fake.location.search).toBe(conflict);
    expect(tree.find((node) => node.getAttribute("role") === "alert")).not.toBeNull();
    expect(tree.elements((node) => node.id.endsWith("-retry"))).toEqual([]);
    expect(tree.find((node) => node.tag === "tbody")).toBeNull();
  });
});

for (const custom of [false, true]) {
  test(`结算目录${custom ? "自定义" : "预设"}页长事件在换址后不得写入旧页长`, async () => {
    const { page } = await loadPage();
    await withAdminTree(`${MAIN}&page=2&auction=${AUCTION_ID}&sequence=5`, async (tree) => {
      const requests: ReturnType<typeof recordRequest>[] = [];
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        requests.push(request);
        if (request.url === REPORT) return new Response(adminContract("settlement-report"));
        if (request.url === RECOVERY) return new Response(RECOVERY_BODY);
        if (request.url === `${CATALOG}?page=2&page_size=10&world_id=main`) {
          return new Response(catalogReply("main", 2, 11, AUCTION_ID).body);
        }
        if (request.url === `${CATALOG}?page=1&page_size=10&world_id=isekai`) {
          return new Response(catalogReply("isekai", 1, 0, null).body);
        }
        throw new Error(`未预期的请求 ${request.url}`);
      });
      const preference = spyOn(pageSizePreference, "save");
      try {
        tree.mount(page);
        await settle();
        await expectReport(tree);
        const region = requireElement(
          tree.find((node) => node.getAttribute("aria-label") === "已结算拍卖分页"),
          "目录分页",
        );
        const selectSize = requireElement(region.querySelector("select"), "每页数量");
        const customForm = requireElement(
          region.querySelector(".ui-pager__custom"),
          "自定义页长表单",
        );
        const field = requireElement(customForm.querySelector("input"), "自定义页长");
        if (custom) field.input("30");
        const apply = () => {
          if (custom) customForm.submit();
          else selectSize.change("20");
        };

        tree.router.push(ISEKAI);
        const previous = [...tree.fake.writes];
        apply();
        expect(tree.fake.writes).toEqual(previous);
        expect(preference).not.toHaveBeenCalled();
        expect(tree.fake.location.searchParams.get("page_size")).toBe("10");
        expect(tree.fake.location.searchParams.get("world")).toBe("isekai");
        await settle();
        expect(select(tree, "settlement-world").value).toBe("isekai");
        expect(
          requests.some(({ url }) => url.includes("page_size=30") || url.includes("page_size=20")),
        ).toBe(false);
      } finally {
        preference.mockRestore();
        restore();
      }
    });
  });
}

test("有效目录页长修改只复位目录页，保留场次与两个报告页码", async () => {
  const { AdminSettlementCatalogPager } = await loadRenderModule<typeof import("./picker")>(
    `${import.meta.dir}/picker.tsx`,
    "AdminSettlementCatalogPager",
  );
  const { useAdminPagination } = await loadRenderModule<
    Pick<typeof import("../pagination"), "useAdminPagination">
  >(new URL("../pagination/index.tsx", import.meta.url).pathname, "useAdminPagination");
  await withAdminTree(`${MAIN}&page=2&auction=${AUCTION_ID}&sequence=5`, async (tree) => {
    const lotPage = createCell(3);
    const recoveryPage = createCell(4);
    const catalogPage = createCell(2);
    const selectedAuctionId = createCell(AUCTION_ID);
    const requestedSequence = createCell<number | null>(5);
    tree.mount(() => {
      const pagination = useAdminPagination("settlement-catalog", ["page_size"], 100, 100_000);
      return createComponent(AdminSettlementCatalogPager, {
        get page() {
          return catalogPage();
        },
        get pageSize() {
          return pagination.size();
        },
        total: 100,
        pagination,
        isCurrent: () => true,
        worldFilter: createCell("main"),
        requestedSequence,
        selectedAuctionId,
        catalogPage,
        lotPage,
        recoveryPage,
        confirmation: createCell<RecoveryConfirmationDraft | null>(null),
        reason: createCell(""),
        actionFeedback: createCell<AdminActionFeedback | null>(null),
        selectionGeneration: createCell(0),
      });
    });
    await settle();
    const size = requireElement(
      tree.find((node) => node.getAttribute("name") === "page_size"),
      "每页数量",
    );
    size.change("20");
    await settle();
    expect([catalogPage.peek(), lotPage.peek(), recoveryPage.peek()]).toEqual([1, 3, 4]);
    expect([selectedAuctionId.peek(), requestedSequence.peek()]).toEqual([AUCTION_ID, 5]);
    const address = tree.fake.location.searchParams;
    expect([
      address.get("page"),
      address.get("page_size"),
      address.get("auction"),
      address.get("sequence"),
    ]).toEqual([null, "20", AUCTION_ID, "5"]);
  });
});

test("空尾页只替换一次页码并保留深链接，迟到的旧空页不能纠正到新地址", async () => {
  const { page } = await loadPage();
  for (const moveFirst of [false, true]) {
    const url = `${MAIN}&page=3&auction=${AUCTION_ID}&sequence=5`;
    await withAdminTree(url, async (tree) => {
      const tail = Promise.withResolvers<Response>();
      const paths: string[] = [];
      const restore = installFixtureFetch(async (input, init) => {
        const { url } = recordRequest(input, init);
        paths.push(url);
        switch (url) {
          case `${CATALOG}?page=3&page_size=10&world_id=main`:
            return tail.promise;
          case `${CATALOG}?page=2&page_size=10&world_id=main`:
            return new Response(catalogReply("main", 2, 11, AUCTION_ID).body);
          case `${CATALOG}?page=1&page_size=10&world_id=isekai`:
            return new Response(catalogReply("isekai", 1, 0, null).body);
          case REPORT:
            return new Response(adminContract("settlement-report"));
          case RECOVERY:
            return new Response(RECOVERY_BODY);
          default:
            throw new Error(`未预期的请求 ${url}`);
        }
      });
      try {
        tree.mount(page);
        await settle();
        const before = tree.fake.writes.length;
        tail.resolve(new Response(catalogReply("main", 3, 11, null).body));
        if (moveFirst) tree.router.push(ISEKAI);
        await settle();
        const changes = tree.fake.writes.slice(before);
        if (moveFirst) {
          expect(select(tree, "settlement-world").value).toBe("isekai");
          expect(tree.fake.location.search).toBe("?world=isekai&page_size=10");
          expect(changes.map(({ mode }) => mode)).toEqual(["push"]);
          expect(paths).not.toContain(`${CATALOG}?page=2&page_size=10&world_id=main`);
        } else {
          const clamps = changes.filter(
            ({ url }, index) =>
              url.searchParams.get("page") === "2" &&
              changes[index - 1]?.url.searchParams.get("page") !== "2",
          );
          expect(clamps.map(({ mode }) => mode)).toEqual(["replace"]);
          expect(clamps[0]?.url.searchParams.get("auction")).toBe(AUCTION_ID);
          expect(clamps[0]?.url.searchParams.get("sequence")).toBe("5");
          expect(changes.some(({ mode }) => mode === "push")).toBe(false);
          await expectReport(tree);
        }
      } finally {
        restore();
      }
    });
  }
});
