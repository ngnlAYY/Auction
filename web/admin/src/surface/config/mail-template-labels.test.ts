// 模板占位符按正文写法展示：与 Rust `placeholders_are_shown_the_way_they_are_written_in_the_body` 对齐。

import { expect, test } from "bun:test";

import { placeholderList } from "./mail-template-labels";

test("占位符按正文里的写法展示", () => {
  expect(placeholderList(["amount", "reference"])).toBe("{amount} {reference}");
  expect(placeholderList([])).toBe("—");
});
