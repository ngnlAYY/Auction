// 账号标记整份目录的分页形状与读取边界。

import { expect, test } from "bun:test";

const { catalogPageShape, MAX_CATALOG_ROWS, PAGE_SIZE } = await import("./load");

/** 按标记页的规则逐页读到最后一页，返回读取的页数与行数；某页形状非法时为 `null`。 */
function readWholeCatalog(total: number): readonly [number, number] | null {
  let rows = 0;
  for (let pageNumber = 1; pageNumber <= MAX_CATALOG_ROWS / PAGE_SIZE + 1; pageNumber += 1) {
    const shape = catalogPageShape(pageNumber, total);
    if (shape === null) {
      return null;
    }
    const [pageRows, lastPage] = shape;
    rows += pageRows;
    if (lastPage) {
      return [pageNumber, rows];
    }
  }
  return null;
}

test("whole_catalog_is_read_page_by_page_at_the_public_maximum：按公开上限逐页读完整份目录", () => {
  expect([PAGE_SIZE, MAX_CATALOG_ROWS]).toEqual([200, 10_200]);
  // 空目录、不满一页、恰好整页、跨页与 offset 上限上的末页：读到全部行，且不多读一页。
  const cases: readonly (readonly [number, number])[] = [
    [0, 1],
    [1, 1],
    [200, 1],
    [201, 2],
    [401, 3],
    [10_000, 50],
    [10_200, 51],
  ];
  for (const [total, pages] of cases) {
    expect(readWholeCatalog(total)).toEqual([pages, total]);
  }
  // 每页按上限读满，只有末页按剩余行数。
  expect(catalogPageShape(2, 401)).toEqual([200, false]);
  expect(catalogPageShape(3, 401)).toEqual([1, true]);
  // offset 上限之外的行读不到：总数超出可读范围、负数或页码非正都失败关闭。
  const invalid: readonly (readonly [number, number])[] = [
    [1, 10_201],
    [1, -1],
    [0, 10],
    [-3, 10],
  ];
  for (const [pageNumber, total] of invalid) {
    expect(catalogPageShape(pageNumber, total)).toBeNull();
  }
});
