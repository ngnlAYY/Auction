// 代理新建表单的初始模式跟随冻结策略。

import { expect, test } from "bun:test";

const { initialProxyMode } = await import("./create");

test("initial_mode_follows_the_frozen_default：初始模式跟随冻结默认值", () => {
  expect(initialProxyMode("slow", true)).toBe("slow");
  expect(initialProxyMode("slow", false)).toBe("fast");
  expect(initialProxyMode("fast", true)).toBe("fast");
  expect(initialProxyMode("fast", false)).toBe("fast");
  expect(initialProxyMode(null, true)).toBeNull();
  expect(initialProxyMode("Slow", true)).toBeNull();
  expect(initialProxyMode("", true)).toBeNull();
});
