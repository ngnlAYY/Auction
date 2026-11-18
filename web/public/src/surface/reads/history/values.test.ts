// 价格历史筛选计数、世界草稿、应用变更与查询门槛的回归用例。

import { expect, test } from "bun:test";

const {
  historyActiveFilterCount,
  historyApplyChangesQuery,
  historyFormFilters,
  historyHasRequestCondition,
  historyQueryErrors,
  historyQueryFromUrl,
  historyResetChangesQuery,
  historyWorldDraftForApply,
  historyWorldDraftValid,
  normalizeHistoryQuery,
} = await import("./values");
const { priceHistoryQueryDefault } = await import("../../../api/model");
const { publicBoardQueryDefault } = await import("../../query");

test("specific_world_counts_as_one_filter_and_starts_the_history_query：具体世界计一项筛选并触发查询", () => {
  for (const world of ["main", "isekai"]) {
    const query = { ...priceHistoryQueryDefault(), world };
    expect(historyActiveFilterCount(query)).toBe(1);
    expect(historyHasRequestCondition(query)).toBe(true);
  }
  expect(historyActiveFilterCount(priceHistoryQueryDefault())).toBe(0);
  expect(historyHasRequestCondition(priceHistoryQueryDefault())).toBe(false);
  expect(historyActiveFilterCount({ ...priceHistoryQueryDefault(), world: "all" })).toBe(0);
  expect(historyHasRequestCondition({ ...priceHistoryQueryDefault(), world: "all" })).toBe(false);
});

test("world_adds_to_the_history_filter_count：世界与其他条件一起累计筛选数", () => {
  const query = {
    ...priceHistoryQueryDefault(),
    world: "isekai",
    name: "Phase Robe",
    buyer: "Buyer",
  };
  expect(historyActiveFilterCount(query)).toBe(3);
  expect(historyHasRequestCondition(query)).toBe(true);
});

test("all_worlds_with_a_name_starts_the_history_query：全部世界加名称仍应查询", () => {
  const query = { ...priceHistoryQueryDefault(), world: null, name: "Phase Robe" };
  expect(historyHasRequestCondition(query)).toBe(true);
});

test("买卖家按原文的 UTF-8 字节数和控制字符判断，避免放宽装备查询", () => {
  const query = { ...priceHistoryQueryDefault(), name: "Phase Robe" };
  expect(historyQueryErrors({ ...query, buyer: `甲${"é".repeat(126)}A` })).toEqual({});
  expect(historyQueryErrors({ ...query, buyer: "甲".repeat(86) })).toEqual({
    buyer: "player_length",
  });
  expect(historyQueryErrors({ ...query, seller: "Bob\t" })).toEqual({ seller: "player_invalid" });
});

test("倒置的深链保留日期并阻止查询，同日区间有效", () => {
  const from = "2026-06-30";
  const to = "2026-06-01";
  const query = normalizeHistoryQuery({
    ...publicBoardQueryDefault(),
    historyFrom: from,
    historyTo: to,
  });
  expect([query.historyFrom, query.historyTo]).toEqual([from, to]);
  expect(historyQueryErrors({ ...priceHistoryQueryDefault(), from, to })).toEqual({
    from: "date_range",
  });
  expect(historyQueryErrors({ ...priceHistoryQueryDefault(), from, to: from })).toEqual({});
  expect(historyQueryErrors({ ...priceHistoryQueryDefault(), from: "1970-01-01" })).toEqual({
    from: "from_date_invalid",
  });
  expect(historyQueryErrors({ ...priceHistoryQueryDefault(), to: "9999-12-30" })).toEqual({
    to: "to_date_invalid",
  });
  expect(historyQueryErrors({ ...priceHistoryQueryDefault(), to: "9999-12-29" })).toEqual({});
});

test("world_changes_sync_the_form_draft：世界变化进入表单草稿投影", () => {
  const all = { ...priceHistoryQueryDefault(), name: "Phase Robe" };
  const main = { ...priceHistoryQueryDefault(), world: "main", name: "Phase Robe" };
  const isekai = { ...main, world: "isekai" };
  expect(historyFormFilters(all).world).toBeNull();
  expect(historyFormFilters(main).world).toBe("main");
  expect(historyFormFilters(isekai).world).toBe("isekai");
});

test("narrowed_world_catalog_invalidates_an_unapplied_world：目录收窄后不提交失效世界", () => {
  const mainOnly = [{ value: "main", disabled: false }];
  expect(historyWorldDraftValid("all", mainOnly, null)).toBe(true);
  expect(historyWorldDraftValid("main", mainOnly, null)).toBe(true);
  expect(historyWorldDraftValid("isekai", mainOnly, null)).toBe(false);
  expect(historyWorldDraftValid("isekai", mainOnly, "isekai")).toBe(true);
  expect(
    historyWorldDraftValid(
      "isekai",
      [
        { value: "main", disabled: false },
        { value: "isekai", disabled: true },
      ],
      "isekai",
    ),
  ).toBe(true);
  expect(
    historyWorldDraftValid(
      "isekai",
      [
        { value: "main", disabled: false },
        { value: "isekai", disabled: true },
      ],
      null,
    ),
  ).toBe(false);
  expect(historyWorldDraftForApply("isekai", mainOnly, "main")).toBe("main");
  expect(historyWorldDraftForApply("isekai", mainOnly, null)).toBe("all");
  expect(historyWorldDraftForApply("main", mainOnly, null)).toBe("main");
});

test("apply_only_writes_when_filters_or_target_page_change：无变化时不写历史", () => {
  const applied = {
    ...priceHistoryQueryDefault(),
    world: "main",
    name: "Phase Robe",
    page: 1,
  };
  const candidate = { ...applied, sort: "price_asc" };
  expect(historyApplyChangesQuery(applied, candidate)).toBe(false);
  expect(historyApplyChangesQuery({ ...applied, page: 2 }, candidate)).toBe(true);
  expect(historyApplyChangesQuery(applied, { ...candidate, world: null })).toBe(true);
  expect(historyApplyChangesQuery(applied, { ...candidate, name: "Other Robe" })).toBe(true);
  for (const change of [
    { from: "2026-01-01" },
    { to: "2026-12-31" },
    { minPrice: "1000" },
    { maxPrice: "2000" },
    { buyer: "Buyer" },
    { seller: "Seller" },
  ]) {
    expect(historyApplyChangesQuery(applied, { ...candidate, ...change })).toBe(true);
  }
});

test("reset_writes_when_only_world_is_applied：只有世界被选时清空会改写地址状态", () => {
  const cleared = {
    ...publicBoardQueryDefault(),
    pageSize: 20,
    movementPageSize: 20,
  };
  const shouldReset = (search: string, board = cleared) =>
    historyResetChangesQuery(historyQueryFromUrl(board, search), board.historyGroup);
  expect(shouldReset("?world=main")).toBe(true);
  expect(shouldReset("")).toBe(false);
  expect(shouldReset("?world=all")).toBe(false);
  expect(shouldReset("?name=Phase+Robe")).toBe(true);
  expect(shouldReset("?buyer=+")).toBe(true);
  expect(shouldReset("?buyer=&buyer=")).toBe(true);
  expect(shouldReset("?buyer=")).toBe(false);
  expect(shouldReset("?history_sort=price_asc", { ...cleared, historySort: "price_asc" })).toBe(
    true,
  );
  expect(shouldReset("?page=2", { ...cleared, page: 2 })).toBe(true);
});

test("重复字段与非法日期仍能从原始链接逐项还原", () => {
  const search = new URLSearchParams([
    ["name", "Robe"],
    ["buyer", "Alice"],
    ["buyer", "Bob"],
    ["seller", "Eve\r"],
    ["from", "2026-02-30"],
  ]);
  const { query, errors, duplicates } = historyQueryFromUrl(
    publicBoardQueryDefault(),
    search.toString(),
  );
  expect(query).toMatchObject({
    name: "Robe",
    buyer: "Alice",
    seller: "Eve\r",
    from: "2026-02-30",
  });
  expect(errors).toEqual({
    buyer: "duplicate",
    seller: "player_invalid",
    from: "from_date_invalid",
  });
  expect(duplicates).toEqual({ buyer: ["Alice", "Bob"] });
  search.delete("buyer");
  search.set("seller", "Eve");
  search.set("from", "2028-02-29");
  expect(historyQueryFromUrl(publicBoardQueryDefault(), search.toString()).errors).toEqual({});
});

test("区间任一端是重复参数时优先保留字段错误", () => {
  const board = publicBoardQueryDefault();
  expect(historyQueryFromUrl(board, "?from=2026-07-31&to=2026-07-01&to=2026-08-01").errors).toEqual(
    { to: "duplicate" },
  );
  expect(historyQueryFromUrl(board, "?min_price=20&max_price=10&max_price=30").errors).toEqual({
    maxPrice: "duplicate",
  });
});
