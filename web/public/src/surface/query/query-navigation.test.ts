// 分页与历史导航时，地址、路由快照和已应用查询保持同步。

import { expect, spyOn, test } from "bun:test";
import { writeQueryPairs } from "@auction/web-shared/net/query";
import { t } from "@auction/web-shared/ui/i18n";
import { flush } from "solid-js";

import {
  dispatch,
  hasClass,
  mountCompetition,
  renderedText,
  waitFor,
} from "../commands/competition-fixture.test";
import { updatePublicPageSize } from "../pagination";
import { readPublicQuery } from "./location";
import { expectCurrentAddress, mountQueryStates } from "./query-state-fixture.test";
import { updatePublicQueryState } from "./state";

test("重复应用相同筛选不写历史，筛选或页码变化仍可后退", () => {
  const mounted = mountQueryStates("/history");
  try {
    mounted.first.push({ ...mounted.first.state.peek() });
    expectCurrentAddress(mounted);
    expect(mounted.pushCount()).toBe(0);

    mounted.first.push({ ...mounted.first.state.peek(), historyName: "Phase Robe" });
    expectCurrentAddress(mounted);
    flush();
    expect(mounted.pushCount()).toBe(1);
    expect(mounted.second.state.peek().historyName).toBe("Phase Robe");

    mounted.first.push({ ...mounted.first.state.peek() });
    expect(
      updatePublicQueryState(
        mounted.first.state,
        (query) => {
          query.historyName = "Phase Robe";
        },
        "push",
      ),
    ).toBe(false);
    expectCurrentAddress(mounted);
    expect(mounted.pushCount()).toBe(1);
    const beforePaging = mounted.location.href;
    mounted.first.push({
      ...mounted.first.state.peek(),
      historyName: "Shade Helmet",
      page: 2,
    });
    expectCurrentAddress(mounted);
    flush();
    expect(mounted.pushCount()).toBe(2);
    expect(mounted.second.state.peek()).toMatchObject({ historyName: "Shade Helmet", page: 2 });
    mounted.back();
    expect(mounted.location.href).toBe(beforePaging);
    expect(mounted.first.state.peek()).toMatchObject({ historyName: "Phase Robe", page: 1 });
    expect(mounted.second.state.peek()).toMatchObject({ historyName: "Phase Robe", page: 1 });
  } finally {
    mounted.close();
  }
});

test.each([
  { search: "?page=2&ref=x", page: 2, literal: false },
  { search: "??page=2&ref=x", page: 1, literal: true },
])("页长切换后前进和后退保持路由、查询与列表同步 $search", ({ search, page, literal }) => {
  const mounted = mountQueryStates(`/${search}`);
  try {
    const previous = readPublicQuery();
    expect(previous.page).toBe(page);
    // 页长控件先用共享入口冻结旧条目，再用页面查询写入新页长。
    writeQueryPairs(
      ["page_size", "movement_page_size"],
      [
        ["page_size", String(previous.pageSize)],
        ["movement_page_size", String(previous.movementPageSize)],
      ],
      "replace",
    );
    const oldAddress = expectCurrentAddress(mounted);
    expect(updatePublicPageSize(mounted.first.state, 50)).toBe(true);
    const newAddress = expectCurrentAddress(mounted);
    expect(mounted.first.state.peek()).toMatchObject({ page: 1, pageSize: 50 });
    flush();
    expect(mounted.second.state.peek()).toMatchObject({ page: 1, pageSize: 50 });
    expect(mounted.visibleRows()).toHaveLength(50);

    mounted.back();
    expect(mounted.router.currentLocationString()).toBe(oldAddress);
    expect(`/${mounted.location.search}`).toBe(oldAddress);
    expect(mounted.location.searchParams.get(literal ? "?page" : "page")).toBe("2");
    expect(mounted.first.state.peek()).toMatchObject({ page, pageSize: previous.pageSize });
    expect(mounted.visibleRows()).toHaveLength(previous.pageSize);
    expect(mounted.visibleRows()[0]).toBe((page - 1) * previous.pageSize + 1);

    mounted.forward();
    expect(mounted.router.currentLocationString()).toBe(newAddress);
    expect(`/${mounted.location.search}`).toBe(newAddress);
    expect(mounted.location.searchParams.get("?page")).toBe(literal ? "2" : null);
    expect(mounted.location.searchParams.get("ref")).toBe("x");
    expect(mounted.first.state.peek()).toMatchObject({ page: 1, pageSize: 50 });
    expect(mounted.visibleRows()).toHaveLength(50);
    expect(mounted.visibleRows()[0]).toBe(1);
  } finally {
    mounted.close();
  }
});

test("后退恢复第 2 页后，页码纠偏立即同步快照和已应用查询", () => {
  const mounted = mountQueryStates("/?page=2&page_size=20");
  try {
    mounted.router.push("/?page_size=50");
    expectCurrentAddress(mounted);
    mounted.back();
    expect(mounted.first.state.peek().page).toBe(2);

    expect(
      updatePublicQueryState(
        mounted.first.state,
        (query) => {
          query.page = 1;
        },
        "replace",
      ),
    ).toBe(true);
    expectCurrentAddress(mounted);
    expect(mounted.location.searchParams.has("page")).toBe(false);
    expect(mounted.first.state.peek()).toMatchObject({ page: 1, pageSize: 20 });
    flush();
    expect(mounted.second.state.peek()).toMatchObject({ page: 1, pageSize: 20 });
    expect(mounted.visibleRows()[0]).toBe(1);
    expect(mounted.pushCount()).toBe(1);
  } finally {
    mounted.close();
  }
});

test("共享写入创建独立的历史条目，后退与前进均同步路由和查询", () => {
  const mounted = mountQueryStates("/?page=2&page_size=20");
  try {
    writeQueryPairs(["page_size"], [["page_size", "50"]], "push");
    expect(mounted.location.search).toBe("?page=2&page_size=50");
    expectCurrentAddress(mounted);
    flush();
    expect(mounted.second.state.peek()).toMatchObject({ page: 2, pageSize: 50 });
    mounted.router.push("/bidlog");
    expectCurrentAddress(mounted);
    mounted.back();
    expect(mounted.router.currentLocationString()).toBe("/?page=2&page_size=50");
    expect(mounted.first.state.peek()).toMatchObject({ page: 2, pageSize: 50 });

    mounted.back();
    expect(mounted.location.search).toBe("?page=2&page_size=20");
    expect(mounted.router.currentLocationString()).toBe("/?page=2&page_size=20");
    expect(mounted.first.state.peek()).toMatchObject({ page: 2, pageSize: 20 });
    expect(mounted.visibleRows()[0]).toBe(21);

    mounted.forward();
    expect(mounted.router.currentLocationString()).toBe("/?page=2&page_size=50");
    expect(mounted.first.state.peek()).toMatchObject({ page: 2, pageSize: 50 });
    expect(mounted.visibleRows()[0]).toBe(51);
  } finally {
    mounted.close();
  }
});

test.each(["push", "replace"] as const)("路由 %s 只在写入成功后同步快照和查询", (mode) => {
  const mounted = mountQueryStates("/?page=2");
  try {
    const previous = mounted.router.current();
    const snapshotUpdates = mounted.snapshotUpdates();
    mounted.failHistoryWrites(true);
    mounted.router[mode]("/bidlog?page=3");
    expect(expectCurrentAddress(mounted)).toBe("/?page=2");
    flush();
    expect(mounted.snapshotUpdates()).toBe(snapshotUpdates);
    expect(mounted.router.current()).toBe(previous);
    expect(mounted.second.state.peek().page).toBe(2);

    mounted.failHistoryWrites(false);
    mounted.router[mode]("/bidlog?page=3");
    expect(expectCurrentAddress(mounted)).toBe("/bidlog?page=3");
    expect(mounted.router.current()).toMatchObject({ kind: "matched", name: "BidLog" });
    flush();
    expect(mounted.second.state.peek().page).toBe(3);
  } finally {
    mounted.close();
  }
});

test("同页重选清除筛选，跨页历史仍恢复世界和列表", () => {
  const mounted = mountQueryStates("/history?world=isekai&name=Phase+Robe&page=3&page_size=20");
  const assertQuery = (
    world: string | null,
    name: string | null,
    page: number,
    firstRow: number,
  ) => {
    for (const query of [mounted.first, mounted.second]) {
      expect(query.state.peek()).toMatchObject({ world, historyName: name, page });
    }
    expect(mounted.visibleRows()[0]).toBe(firstRow);
  };
  try {
    assertQuery("isekai", "Phase Robe", 3, 41);
    mounted.router.push("/history?world=isekai");
    flush();
    assertQuery("isekai", null, 1, 1);
    mounted.router.push("/bidlog?world=main");
    flush();
    assertQuery("main", null, 1, 1);
    mounted.back();
    assertQuery("isekai", null, 1, 1);
    mounted.back();
    assertQuery("isekai", "Phase Robe", 3, 41);
    mounted.forward();
    assertQuery("isekai", null, 1, 1);
  } finally {
    mounted.close();
  }
});

test("冻结旧页长、拒绝新页长并重试时每个 Public 实例只经原有路由解析", () => {
  const mounted = mountQueryStates("/?page=3&ref=x");
  const getAll = spyOn(URLSearchParams.prototype, "getAll");
  const parsedQueries = () => getAll.mock.calls.filter(([key]) => key === "auction").length;
  try {
    expect(
      writeQueryPairs(
        ["page_size", "movement_page_size"],
        [
          ["page_size", "20"],
          ["movement_page_size", "20"],
        ],
        "replace",
      ),
    ).toBe(true);
    flush();
    expect(parsedQueries()).toBe(2);
    mounted.failHistoryWrites(true);
    expect(updatePublicPageSize(mounted.first.state, 50)).toBe(false);
    flush();
    expect(parsedQueries()).toBe(2);
    expect(mounted.location.search).toBe("?page=3&ref=x&page_size=20&movement_page_size=20");
    for (const query of [mounted.first, mounted.second]) {
      expect(query.state.peek()).toMatchObject({ page: 3, pageSize: 20 });
    }
    expect(mounted.visibleRows()[0]).toBe(41);

    mounted.failHistoryWrites(false);
    expect(updatePublicPageSize(mounted.first.state, 50)).toBe(true);
    flush();
    // 提交者同步读回新地址，两份路由订阅随后各解析一次。
    expect(parsedQueries()).toBe(5);
    for (const query of [mounted.first, mounted.second]) {
      expect(query.state.peek()).toMatchObject({ page: 1, pageSize: 50 });
    }
    expect(mounted.visibleRows()).toHaveLength(50);
    expect(mounted.visibleRows()[0]).toBe(1);
  } finally {
    getAll.mockRestore();
    mounted.close();
  }
});

test("点击 Public 侧栏当前页恢复历史价筛选，跨页后退恢复外壳世界", async () => {
  const original = "/history?world=isekai&name=Phase+Robe&page=3&page_size=20";
  const page = await mountCompetition("isekai", false, { href: original });
  const nameInput = () => page.find((node) => node.getAttribute("id") === "public-history-name");
  const nameValue = () => nameInput()?.attributes.value;
  const sidebar = () => page.need((node) => hasClass(node, "console-sidebar"));
  const world = () => renderedText(page.find((node) => hasClass(node, "console-ticker__world")));
  try {
    await waitFor(() => nameValue() === "Phase Robe");
    const currentPage = nameInput();
    const current = page.need(
      (node) => node.tag === "a" && node.getAttribute("href") === "/history",
      sidebar(),
    );
    dispatch(current, "Click");
    await waitFor(() => nameValue() === "");
    expect(nameInput()).toBe(currentPage);
    expect(page.location.pathname + page.location.search).toBe("/history");

    page.go("/bidlog?world=isekai");
    await waitFor(() => world() === ` / ${t("auction-world-isekai")}`);
    page.go("/status?world=main");
    await waitFor(() => world() === ` / ${t("auction-world-main")}`);
    page.back();
    await waitFor(() => world() === ` / ${t("auction-world-isekai")}`);
    expect(page.location.pathname).toBe("/bidlog");
    page.back();
    await waitFor(() => nameValue() === "");
    expect(page.location.pathname).toBe("/history");
    page.back();
    await waitFor(() => nameValue() === "Phase Robe");
    expect(page.location.pathname + page.location.search).toBe(original);
  } finally {
    page.dispose();
  }
});
