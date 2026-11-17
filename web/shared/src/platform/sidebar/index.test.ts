// 控制台侧栏偏好闭集的回归用例，逐条对照 Rust `platform::sidebar` 的测试。

import { expect, test } from "bun:test";
import { parse } from "./index";

test("sidebar_preference_is_a_closed_set：侧栏偏好只接受 collapsed", () => {
  expect(parse("collapsed")).toBe(true);
  expect(parse("expanded")).toBe(false);
  expect(parse(null)).toBe(false);
  expect(parse("")).toBe(false);
  expect(parse("true")).toBe(false);
  expect(parse("Collapsed")).toBe(false);
});
