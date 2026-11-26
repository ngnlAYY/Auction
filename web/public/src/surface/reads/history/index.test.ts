// 价格历史页面对深链校验、修正后请求，以及拒绝写入后的表单恢复。

import { expect, spyOn, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import { For, flush } from "solid-js";

import { resetStoreForTest } from "../../../api/swr-cache";
import { mountReadPage } from "../../pagination/browser-fixture";
import { usePublicQueryState } from "../../query";
import { History } from ".";
import { PriceHistoryFilters } from "./filters/components";
import { useHistoryFilters } from "./filters/state";
import { HistoryResultControls } from "./presentation";
import { PriceHistoryTable } from "./table";

function errorCodes(filters: ReturnType<typeof useHistoryFilters>) {
  return Object.fromEntries(
    Object.entries(filters.errors.peek()).map(([field, issue]) => [field, issue.code]),
  );
}

test.each([
  ["min_price=20&max_price=30", "maxPrice", { maximumPrice: "10" }, "minPrice", "price_range"],
  ["from=2026-07-01&to=2026-07-31", "to", { toMonth: "06" }, "from", "date_range"],
] as const)("区间上界失焦即显示倒置，修复后恢复 %s", (search, upper, patch, lower, code) => {
  const probe: { filters?: ReturnType<typeof useHistoryFilters> } = {};
  const mounted = mountReadPage(`/history?${search}`, () => {
    probe.filters = useHistoryFilters(usePublicQueryState().state, () => []);
  });
  try {
    const filters = probe.filters;
    if (filters === undefined) throw new Error("未装配筛选");
    filters.updateField(upper, patch);
    expect(errorCodes(filters)).toEqual({});
    expect(filters.blur(upper)).toBe(code);
    expect(errorCodes(filters)).toEqual({ [lower]: code });
    expect(filters.interacted.peek()).toContain(lower);

    if (upper === "maxPrice") filters.updateField(upper, { maximumPrice: "30" });
    else filters.updateField(upper, { toMonth: "08" });
    expect(filters.blur(upper)).toBeNull();
    expect(errorCodes(filters)).toEqual({});
    expect(mounted.location.search).toBe(`?${search}`);
  } finally {
    mounted.dispose();
  }
});

test("日期失焦后的提示回显本次校验的端点", () => {
  let renderForm: () => unknown = () => null;
  let form: unknown;
  const mounted = mountReadPage(
    "/history?from=2026-07-01&to=2026-07-31",
    () => {
      const filters = useHistoryFilters(usePublicQueryState().state, () => []);
      renderForm = () => PriceHistoryFilters({ filters, worldOptions: [], activeFilterCount: 2 });
      form = renderForm();
    },
    ({ tag, props }) => {
      if (typeof tag === "function" && tag.name === "HistoryDateFilter") {
        Reflect.apply(tag, undefined, [props]);
      }
    },
  );
  try {
    const toggle = mounted
      .visible(form)
      .find(
        ({ tag, props }) =>
          tag === "button" && props["aria-controls"] === "public-history-settings",
      )?.props.onClick;
    if (typeof toggle !== "function") throw new Error("未装配筛选设置开关");
    toggle();
    const first = mounted.elements.length;
    form = mounted.run(renderForm);
    expect(mounted.visible(form).some(({ props }) => props.id === "public-history-settings")).toBe(
      true,
    );
    const openForm = mounted.elements.slice(first);
    const month = openForm.find(
      ({ tag, props }) => tag === "select" && props.name === "from_month",
    )?.props;
    const error = openForm.find(
      ({ tag, props }) =>
        typeof tag === "function" &&
        tag.name === "HistoryFieldError" &&
        props.feedback !== null &&
        typeof props.feedback === "object" &&
        "field" in props.feedback &&
        props.feedback.field === "from",
    );
    if (
      typeof month?.onChange !== "function" ||
      typeof month.onBlur !== "function" ||
      typeof error?.tag !== "function"
    )
      throw new Error("未装配起始日期字段");
    month.onChange({ currentTarget: { value: "09" } });
    month.onBlur();
    flush();

    const render = error.tag;
    const tree = mounted.run(() => Reflect.apply(render, undefined, [error.props]));
    const errors = mounted.visible(tree);
    expect(
      errors.find(({ props }) => props.id === "public-history-from-error")?.children,
    ).toContain(t("history-filter-error-date-range"));
    expect(errors.filter(({ tag }) => tag === "bdi").flatMap(({ children }) => children)).toEqual([
      '"2026-09-01"',
    ]);
    expect(mounted.location.searchParams.get("from")).toBe("2026-07-01");
  } finally {
    mounted.dispose();
  }
});

async function settleRead() {
  await Bun.sleep(0);
  flush();
  await Bun.sleep(0);
  flush();
}

test.each([
  [`buyer=${"甲".repeat(86)}`, "buyer", "Alice"],
  ["buyer=Alice&buyer=Other", "buyer", "Alice"],
  ["from=2026-07-31&to=2026-07-01", "from", "2026-07-01"],
])("非法深链 %s 阻止成交查询，修正后才发 GET", async (filter, key, fixed) => {
  resetStoreForTest();
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(async () => new Response("{}", { status: 503 }), { preconnect() {} }),
  );
  const mounted = mountReadPage(`/history?name=Robe&${filter}`, History);
  const requests = () =>
    fetcher.mock.calls
      .map(([input]) => new URL(String(input), mounted.location))
      .filter((url) => url.pathname === "/api/v1/price-history");
  try {
    await settleRead();
    expect(requests()).toHaveLength(0);
    expect(mounted.location.searchParams.get("name")).toBe("Robe");

    const corrected = new URL(mounted.location);
    corrected.searchParams.set(key, fixed);
    mounted.router.replace(corrected.pathname + corrected.search);
    await settleRead();

    expect(requests()).toHaveLength(1);
    const request = requests()[0];
    expect(request?.searchParams.getAll(key)).toEqual([fixed]);
    expect(request?.searchParams.get("name")).toBe("Robe");
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});

test("表单保留非法买家原文，清除或应用被拒时恢复实际地址中的筛选", () => {
  const probe: { filters?: ReturnType<typeof useHistoryFilters> } = {};
  const mounted = mountReadPage("/history?buyer=Alice&name=Robe", () => {
    probe.filters = useHistoryFilters(usePublicQueryState().state, () => []);
  });
  try {
    const filters = probe.filters;
    if (filters === undefined) throw new Error("未装配筛选");
    const illegal = "甲".repeat(86);
    filters.updateField("buyer", { buyer: illegal });
    expect(filters.apply()).toBe("buyer");
    expect(filters.draft.peek().buyer).toBe(illegal);
    expect(errorCodes(filters)).toEqual({ buyer: "player_length" });
    expect(mounted.location.searchParams.get("buyer")).toBe("Alice");

    mounted.rejectWrites(true);
    filters.updateField("buyer", { buyer: "Bob" });
    filters.apply();
    expect(filters.draft.peek().buyer).toBe("Alice");
    filters.reset();
    expect(filters.draft.peek()).toMatchObject({ buyer: "Alice", name: "Robe" });

    mounted.rejectWrites(false);
    filters.updateField("buyer", { buyer: "Bob" });
    filters.apply();
    flush();
    expect(mounted.location.searchParams.get("buyer")).toBe("Bob");
    filters.reset();
    flush();
    expect(mounted.location.searchParams.get("buyer")).toBeNull();
    expect(filters.draft.peek()).toMatchObject({ buyer: "", name: "" });
  } finally {
    mounted.dispose();
  }
});

test("成交历史分组、排序、翻页各自修改地址键并保持查询条件", async () => {
  resetStoreForTest();
  const requests: string[] = [];
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: RequestInfo | URL) => {
        const url = new URL(String(input), "https://auction.test");
        if (url.pathname === "/api/v1/price-history") requests.push(url.search);
        return new Response("{}", { status: 503 });
      },
      { preconnect() {} },
    ),
  );
  const raw = "name=%52obe&buyer=+&other=x%2fy&other=%7e&bare";
  const mounted = mountReadPage(
    `/history?${raw}&page=3&page_size=10#results`,
    History,
    ({ tag, props }) => {
      if (typeof tag === "function" && tag === HistoryResultControls) {
        Reflect.apply(tag, undefined, [props]);
      }
    },
  );
  try {
    await settleRead();
    expect(requests).toEqual(["?name=Robe&buyer=+&sort=closed_desc&page=3&page_size=10"]);
    const group = mounted.elements.find(
      ({ tag, props }) => tag === "select" && props.name === "history_group",
    )?.props.onChange;
    const sort = mounted.elements.find(
      ({ tag, props }) => tag === "select" && props.name === "history_sort",
    )?.props.onChange;
    if (typeof group !== "function" || typeof sort !== "function")
      throw new Error("未挂载成交控件");

    const groupSelect = { value: "buyer" };
    const sortSelect = { value: "price_desc" };
    mounted.rejectWrites("push");
    group({ currentTarget: groupSelect });
    sort({ currentTarget: sortSelect });
    expect(groupSelect.value).toBe("flat");
    expect(sortSelect.value).toBe("closed_desc");
    expect(mounted.location.search).toBe(`?${raw}&page=3&page_size=10`);
    await settleRead();
    expect(requests).toHaveLength(1);

    mounted.rejectWrites(false);
    groupSelect.value = "buyer";
    group({ currentTarget: groupSelect });
    await settleRead();
    expect(mounted.location.search).toBe(`?${raw}&page=3&page_size=10&history_group=buyer`);
    expect(requests).toHaveLength(1);

    sortSelect.value = "price_desc";
    sort({ currentTarget: sortSelect });
    await settleRead();
    expect(mounted.location.search).toBe(
      `?${raw}&page=3&page_size=10&history_group=buyer&history_sort=price_desc`,
    );

    const tableItem = mounted.elements.find(({ tag }) => tag === For)?.children[0];
    if (typeof tableItem !== "function") throw new Error("未挂载成交列表");
    tableItem(() => ({ key: "test", page: { rows: [], page: 3, page_size: 10, total: 40 } }));
    const next = mounted.elements.find(({ tag }) => tag === PriceHistoryTable)?.props.onPageChange;
    if (typeof next !== "function") throw new Error("未挂载成交分页");
    next(4);
    await settleRead();
    expect(mounted.location.search).toBe(
      `?${raw}&page_size=10&history_group=buyer&history_sort=price_desc&page=4`,
    );
    expect(requests).toEqual([
      "?name=Robe&buyer=+&sort=closed_desc&page=3&page_size=10",
      "?name=Robe&buyer=+&sort=price_desc&page=3&page_size=10",
      "?name=Robe&buyer=+&sort=price_desc&page=4&page_size=10",
    ]);
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});

test("成交历史排序写入与前进后退同步已应用查询，保留无关筛选原文", () => {
  resetStoreForTest();
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(async () => new Response("{}", { status: 503 }), { preconnect() {} }),
  );
  const raw = "name=%52obe&auction=%74runcated&auction=bad%0a&other=%2f&bare";
  const initial = `/history?${raw}&history_%73ort=closed_desc#results`;
  const next = `/history?${raw}&history_sort=price_desc#results`;
  const query: { state?: ReturnType<typeof usePublicQueryState>["state"] } = {};
  const mounted = mountReadPage(
    initial,
    () => {
      query.state = usePublicQueryState().state;
      History();
    },
    ({ tag, props }) => {
      if (typeof tag === "function" && tag === HistoryResultControls) {
        Reflect.apply(tag, undefined, [props]);
      }
    },
  );
  try {
    const sort = mounted.elements.find(
      ({ tag, props }) => tag === "select" && props.name === "history_sort",
    )?.props.onChange;
    if (typeof sort !== "function") throw new Error("未装配成交排序");
    const expectApplied = (address: string, historySort: string) => {
      expect(mounted.location.href).toBe(`https://auction.test${address}`);
      expect(mounted.router.currentLocationString()).toBe(address);
      expect(query.state?.peek()).toMatchObject({ historySort, historyName: "Robe" });
    };

    expectApplied(initial, "closed_desc");
    sort({ currentTarget: { value: "price_desc" } });
    expect(mounted.router.currentLocationString()).toBe(next);
    flush();
    expectApplied(next, "price_desc");
    mounted.back();
    expectApplied(initial, "closed_desc");
    mounted.forward();
    expectApplied(next, "price_desc");
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});

test("仅有空格买家的原始筛选也需要写址才能清除", async () => {
  resetStoreForTest();
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(async () => new Response("{}", { status: 503 }), { preconnect() {} }),
  );
  const mounted = mountReadPage("/history?buyer=+&other=%2f", History, ({ tag, props }) => {
    if (typeof tag === "function" && tag === PriceHistoryFilters) {
      Reflect.apply(tag, undefined, [props]);
    }
  });
  try {
    const filters = mounted.elements.find(({ tag }) => tag === PriceHistoryFilters)?.props.filters;
    const clear = mounted.elements.find(
      ({ tag, props }) =>
        tag === "button" && props.class === "public-history-filter-actions__clear",
    )?.props.onClick;
    if (typeof filters !== "object" || filters === null || typeof clear !== "function") {
      throw new Error("未装配清除筛选控件");
    }
    const draft = Reflect.get(filters, "draft");
    if (typeof draft !== "function") throw new Error("未装配筛选草稿");
    await settleRead();
    expect(draft()).toMatchObject({ buyer: " " });
    expect(fetcher.mock.calls.map(([input]) => String(input))).toEqual([
      "/api/v1/price-history?buyer=+&sort=closed_desc&page=1&page_size=20",
    ]);
    mounted.rejectWrites("push");
    clear();
    await settleRead();
    expect(draft()).toMatchObject({ buyer: " " });
    expect(mounted.location.search).toBe("?buyer=+&other=%2f");
    expect(fetcher).toHaveBeenCalledTimes(1);

    mounted.rejectWrites(false);
    clear();
    await settleRead();
    expect(draft()).toMatchObject({ buyer: "" });
    expect(mounted.location.search).toBe("?other=%2f");
    // 清空后没有任何条件，页面回到全部成交的第一页。
    expect(fetcher.mock.calls.map(([input]) => String(input))).toEqual([
      "/api/v1/price-history?buyer=+&sort=closed_desc&page=1&page_size=20",
      "/api/v1/price-history?sort=closed_desc&page=1&page_size=20",
    ]);
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});

test("修正区间后仍要求用户处理未编辑的重复项和非法格式", () => {
  const probe: { filters?: ReturnType<typeof useHistoryFilters> } = {};
  const mounted = mountReadPage(
    "/history?from=2026-07-31&to=2026-07-01&buyer=A&buyer=B&min_price=020",
    () => {
      probe.filters = useHistoryFilters(usePublicQueryState().state, () => []);
    },
  );
  try {
    const filters = probe.filters;
    if (filters === undefined) throw new Error("未装配筛选");
    filters.updateField("to", { toMonth: "08" });
    expect(filters.draft.peek().minimumPrice).toBe("020");
    expect(errorCodes(filters)).toEqual({ minPrice: "minimum_price_c", buyer: "duplicate" });
    expect(filters.apply()).toBe("minPrice");
    expect(errorCodes(filters)).toEqual({ minPrice: "minimum_price_c", buyer: "duplicate" });
    expect(mounted.location.searchParams.getAll("buyer")).toEqual(["A", "B"]);
    expect(mounted.location.searchParams.get("min_price")).toBe("020");
    filters.updateField("minPrice", { minimumPrice: "20" });
    expect(filters.apply()).toBe("buyer");
    filters.updateField("buyer", { buyer: "B" });
    expect(filters.apply()).toBeNull();
    flush();
    expect(mounted.location.searchParams.getAll("buyer")).toEqual(["B"]);
    expect(mounted.location.searchParams.get("to")).toBe("2026-08-31");
  } finally {
    mounted.dispose();
  }
});

test("金额格式错误在区间校验之前生效，另一端的编辑和失焦不会清掉它", () => {
  const probe: { filters?: ReturnType<typeof useHistoryFilters> } = {};
  const mounted = mountReadPage("/history?min_price=020&max_price=10", () => {
    probe.filters = useHistoryFilters(usePublicQueryState().state, () => []);
  });
  try {
    const filters = probe.filters;
    if (filters === undefined) throw new Error("未装配筛选");
    expect(errorCodes(filters)).toEqual({ minPrice: "minimum_price_c" });
    expect(filters.apply()).toBe("minPrice");
    expect(errorCodes(filters)).toEqual({ minPrice: "minimum_price_c" });
    filters.updateField("maxPrice", { maximumPrice: "30" });
    filters.blur("maxPrice");
    expect(errorCodes(filters)).toEqual({ minPrice: "minimum_price_c" });

    filters.updateField("maxPrice", { maximumPrice: "10" });
    expect(errorCodes(filters)).toEqual({ minPrice: "minimum_price_c" });
    expect(filters.apply()).toBe("minPrice");
    expect(errorCodes(filters)).toEqual({ minPrice: "minimum_price_c" });
    expect(mounted.location.search).toBe("?min_price=020&max_price=10");

    filters.updateField("minPrice", { minimumPrice: "20" });
    expect(filters.blur("minPrice")).toBe("price_range");
    expect(errorCodes(filters)).toEqual({ minPrice: "price_range" });
  } finally {
    mounted.dispose();
  }
});

test.each([
  ["from=2026-07-31&to=2026-07-01", "to", { toMonth: "08" }, "2026-08-31"],
  ["min_price=20&max_price=10", "maxPrice", { maximumPrice: "30" }, "30"],
] as const)("只修正区间一端即可清除区间错误 %s", (search, field, patch, value) => {
  const probe: { filters?: ReturnType<typeof useHistoryFilters> } = {};
  const mounted = mountReadPage(`/history?${search}`, () => {
    probe.filters = useHistoryFilters(usePublicQueryState().state, () => []);
  });
  try {
    const filters = probe.filters;
    if (filters === undefined) throw new Error("未装配筛选");
    expect(filters.apply()).toBe(field === "to" ? "from" : "minPrice");
    filters.updateField(field, patch);
    expect(filters.apply()).toBeNull();
    flush();
    expect(mounted.location.searchParams.get(field === "to" ? "to" : "max_price")).toBe(value);
  } finally {
    mounted.dispose();
  }
});

test("没有任何筛选条件时直接读取全部成交的第一页，取回后装配结果并保持缺省地址", async () => {
  resetStoreForTest();
  const row = {
    auction_id: "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b",
    auction_sequence: 5,
    buyer_name: "buyer",
    closed_at: "2025-11-14T12:50:40.11Z",
    equipment_category: "Staff",
    equipment_level: 450,
    equipment_name: "Legendary Staff",
    lot_id: "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c",
    lot_number: 1,
    price: 2000,
    seller_name: "seller",
    world_id: "main",
  };
  const body = JSON.stringify({
    code: "ok",
    message: "价格历史已加载",
    data: {
      page: 1,
      page_size: 20,
      rows: [row],
      stats: { count: 1, min: 2000, max: 2000, median: 2000, median_exact: "2000" },
      total: 1,
    },
  });
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(async () => new Response(body, { status: 200 }), { preconnect() {} }),
  );
  const mounted = mountReadPage("/history", History);
  const requests = () =>
    fetcher.mock.calls
      .map(([input]) => new URL(String(input), mounted.location))
      .filter((url) => url.pathname === "/api/v1/price-history")
      .map((url) => url.search);
  try {
    // 发出读取后顶栏显示加载；取回与页码吻合的第一页后清除，说明结果已装配到表格分支。
    expect(mounted.loading.peek().size).toBe(1);
    await settleRead();
    expect(requests()).toEqual(["?sort=closed_desc&page=1&page_size=20"]);
    expect(mounted.loading.peek().size).toBe(0);
    expect(mounted.location.search).toBe("");
    const controls = mounted.elements.find(({ tag }) => tag === HistoryResultControls)?.props;
    expect(controls?.grouping).toBe("flat");
    const table = mounted.elements.find(({ tag }) => tag === For)?.children[0];
    if (typeof table !== "function") throw new Error("未装配成交列表");
    const page = JSON.parse(body).data;
    table(() => ({ key: "first", page }));
    const rendered = mounted.elements.find(({ tag }) => tag === PriceHistoryTable)?.props;
    expect(rendered).toMatchObject({ grouping: "flat", currentPage: 1, page: { total: 1 } });
  } finally {
    mounted.dispose();
    fetcher.mockRestore();
    resetStoreForTest();
  }
});
