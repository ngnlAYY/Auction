// 公开装备分类多选值的解析、拆分与切换断言。

import { expect, test } from "bun:test";

import {
  catalogCategoryCloseOutcome,
  catalogCategoryCommitValue,
  catalogCategoryEscapeAction,
  catalogCategoryKeys,
  normalizeCatalogCategories,
  shouldRestoreCatalogCategoryFocus,
  toggleCatalogCategory,
} from "./category";

test("single_and_multiple_categories_normalize_in_contract_order：单值与多值都按分类表顺序规范化", () => {
  expect(normalizeCatalogCategories("one")).toBe("one");
  expect(normalizeCatalogCategories("sta,one")).toBe("one,sta");
  expect(normalizeCatalogCategories("hea,clo,one")).toBe("one,clo,hea");
});

test("duplicates_empty_items_and_unknown_keys_are_rejected：重复、空项与未知键一律拒绝", () => {
  for (const value of ["", "one,", "one,,sta", ",one", "one,one", "one,all", "one,unknown"]) {
    expect(normalizeCatalogCategories(value)).toBeNull();
  }
});

test("category_keys_treat_all_and_null_as_no_filter：全部与缺省都表示空分类集合", () => {
  expect(catalogCategoryKeys(null)).toEqual([]);
  expect(catalogCategoryKeys("all")).toEqual([]);
  expect(catalogCategoryKeys("one,sta")).toEqual(["one", "sta"]);
});

test("toggle_adds_removes_and_returns_to_all_when_empty：切换可增删并在删空时回到全部", () => {
  expect(toggleCatalogCategory("all", "sta")).toBe("sta");
  expect(toggleCatalogCategory("sta", "one")).toBe("one,sta");
  expect(toggleCatalogCategory("one,sta", "sta")).toBe("one");
  expect(toggleCatalogCategory("one", "one")).toBe("all");
});

test("commit_only_changes_on_close_and_normalizes_the_draft：关闭时只在草稿变化后提交规范化值", () => {
  expect(catalogCategoryCommitValue("all", "all")).toBeNull();
  expect(catalogCategoryCommitValue("clo,lig", "lig,clo")).toBeNull();
  expect(catalogCategoryCommitValue("all", "lig,clo")).toBe("clo,lig");
  expect(catalogCategoryCommitValue("one,sta", "sta")).toBe("sta");
  expect(catalogCategoryCommitValue("one,sta", "all")).toBe("all");
});

test("close_outcome_reopens_and_clears_focus_request_after_write_failure：写入失败保留面板与草稿", () => {
  expect(catalogCategoryCloseOutcome("sta", false)).toEqual({
    panelOpen: true,
    focusRequest: false,
    focusToggleNow: false,
  });
  expect(catalogCategoryCloseOutcome("sta", true)).toEqual({
    panelOpen: false,
    focusRequest: true,
    focusToggleNow: false,
  });
  expect(catalogCategoryCloseOutcome(null, false)).toEqual({
    panelOpen: false,
    focusRequest: false,
    focusToggleNow: true,
  });
});

test("escape_cancels_hidden_draft_without_a_rendered_toggle：不可见时不提交隐藏草稿", () => {
  expect(catalogCategoryEscapeAction(0)).toBe("cancel");
  expect(catalogCategoryEscapeAction(1)).toBe("submit");
});

test("focus_returns_only_while_document_focus_stays_on_body：只有焦点仍停在文档起点时归还", () => {
  const body = {};
  expect(shouldRestoreCatalogCategoryFocus(body, body)).toBe(true);
  expect(shouldRestoreCatalogCategoryFocus({}, body)).toBe(false);
  expect(shouldRestoreCatalogCategoryFocus(null, body)).toBe(false);
});
