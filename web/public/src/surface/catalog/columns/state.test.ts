// 目录列位集与布局的可见性语义：全选、取消全选和跨布局偏好。

import { expect, test } from "bun:test";

const {
  catalogLayoutColumns,
  catalogVisibilityContains,
  catalogVisibilityDefault,
  catalogVisibilityVisibleCount,
  catalogVisibilityWithAllColumns,
  catalogVisibilityWithColumn,
  catalogVisibilityWithOnlyItem,
} = await import("./state");

test("select_all_shows_every_column_of_the_layout：全选显示布局的全部列", () => {
  const all = catalogVisibilityWithAllColumns(catalogVisibilityDefault(), "live");
  expect(catalogVisibilityContains(all, "group")).toBe(true);
  expect(catalogVisibilityVisibleCount(all, "live")).toBe(catalogLayoutColumns("live").length);
});

test("clearing_select_all_keeps_only_the_item_column：取消全选只保留装备列", () => {
  const all = catalogVisibilityWithAllColumns(catalogVisibilityDefault(), "live");
  const cleared = catalogVisibilityWithOnlyItem(all, "live");
  expect(catalogVisibilityVisibleCount(cleared, "live")).toBe(1);
  expect(catalogVisibilityContains(cleared, "item")).toBe(true);
  // 目录始终至少保留一列：最后一列不能再被隐藏。
  expect(catalogVisibilityWithColumn(cleared, "live", "item", false)).toEqual(cleared);
});

test("clearing_select_all_preserves_columns_outside_the_layout：取消全选保留布局之外的列", () => {
  // 预览布局没有“下次最低价”列；在预览里取消全选不能丢掉竞价布局对它的偏好。
  const all = catalogVisibilityWithAllColumns(catalogVisibilityDefault(), "live");
  const cleared = catalogVisibilityWithOnlyItem(all, "preview");
  expect(catalogVisibilityContains(cleared, "next_bid")).toBe(true);
  expect(catalogVisibilityVisibleCount(cleared, "preview")).toBe(1);
});
