// Public 页大小偏好解析：URL 与本地存储共用严格规则，超过上限一律非法。

import { expect, test } from "bun:test";

const { parsePublicPageSize } = await import("./preferences");

test("stored_and_linked_page_sizes_above_the_maximum_are_invalid：存储与链接页长超过上限均非法", () => {
  // URL 与本地偏好共用同一解析：超过上限的旧链接或旧偏好按非法值处理，由调用方恢复默认值。
  expect(parsePublicPageSize("200")).toBe(200);
  expect(parsePublicPageSize("1")).toBe(1);
  for (const value of ["201", "500", "9223372036854775807", "99999999999999999999"]) {
    expect(parsePublicPageSize(value)).toBeNull();
  }
  for (const value of ["", "0", "+20", "-20", "20.0", "2e1", " 20", "20 "]) {
    expect(parsePublicPageSize(value)).toBeNull();
  }
});
