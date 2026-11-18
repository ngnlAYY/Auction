// 价格历史筛选计数与查询门槛的回归用例。

import { expect, test } from "bun:test";

const {
  historyActiveFilterCount,
  historyFormFilters,
  historyHasRequestCondition,
  historyResetChangesQuery,
} = await import("./values");
const { priceHistoryQueryDefault } = await import("../../../api/model");
const { publicBoardQueryDefault } = await import("../../query");

test("specific_world_alone_is_not_counted_but_still_starts_the_history_query：只选世界时筛选数为零但仍需查询", () => {
  for (const world of ["main", "isekai"]) {
    const query = { ...priceHistoryQueryDefault(), world };
    expect(historyActiveFilterCount(query)).toBe(0);
    expect(historyHasRequestCondition(query)).toBe(true);
  }
  expect(historyHasRequestCondition(priceHistoryQueryDefault())).toBe(false);
  expect(historyHasRequestCondition({ ...priceHistoryQueryDefault(), world: "all" })).toBe(false);
});

test("world_does_not_add_to_the_history_filter_count：世界不增加本页筛选数", () => {
  const query = {
    ...priceHistoryQueryDefault(),
    world: "isekai",
    name: "Phase Robe",
    buyer: "Buyer",
  };
  expect(historyActiveFilterCount(query)).toBe(2);
  expect(historyHasRequestCondition(query)).toBe(true);
});

test("all_worlds_with_a_name_starts_the_history_query：全部世界加名称仍应查询", () => {
  const query = { ...priceHistoryQueryDefault(), world: null, name: "Phase Robe" };
  expect(historyHasRequestCondition(query)).toBe(true);
});

test("world_changes_do_not_touch_the_form_draft：世界范围不参与草稿同步", () => {
  const main = { ...priceHistoryQueryDefault(), world: "main", name: "Phase Robe" };
  const isekai = { ...main, world: "isekai" };
  expect(historyFormFilters(main)).toEqual(historyFormFilters(isekai));
});

test("reset_is_a_noop_when_only_world_and_page_sizes_are_applied：无可清项时不改写地址状态", () => {
  const applied = {
    ...publicBoardQueryDefault(),
    world: "main",
    pageSize: 20,
    movementPageSize: 20,
  };
  expect(historyResetChangesQuery(applied)).toBe(false);
  expect(historyResetChangesQuery({ ...applied, historyName: "Phase Robe" })).toBe(true);
  expect(historyResetChangesQuery({ ...applied, historySort: "price_asc" })).toBe(true);
  expect(historyResetChangesQuery({ ...applied, page: 2 })).toBe(true);
});
