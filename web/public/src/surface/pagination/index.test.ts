// Public 分页数量的接受范围：预设与默认值停在契约上限。

import { expect, test } from "bun:test";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";

const { acceptedPublicPageSize, DEFAULT_PAGE_SIZE, normalizePublicPageSize, PUBLIC_PAGE_SIZES } =
  await import(".");

test("page_size_choices_stop_at_the_contract_maximum：每页数量选择停在契约上限", () => {
  expect(MAX_PUBLIC_PAGE_SIZE).toBe(200);
  for (const size of [1, 37, 200, ...PUBLIC_PAGE_SIZES]) {
    expect(acceptedPublicPageSize(size)).toBe(size);
    expect(normalizePublicPageSize(size)).toBe(size);
  }
  // 自定义输入 201 等越界数量不被接受：分页器不写 URL、不保存偏好，也就不会发出请求。
  for (const size of [201, 1_000, 2 ** 63 - 1, 0, -1, -(2 ** 63)]) {
    expect(acceptedPublicPageSize(size)).toBeNull();
    // 其他入口拿到越界值时回退默认值，不截断成上限。
    expect(normalizePublicPageSize(size)).toBe(DEFAULT_PAGE_SIZE);
  }
});
