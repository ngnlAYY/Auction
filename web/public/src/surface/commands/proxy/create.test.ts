// 代理新建表单的初始模式跟随冻结策略，上限输入预填以 K 为单位的纯数字。

import { expect, test } from "bun:test";

const { initialProxyMode } = await import("./create");
const { initialProxyAmount } = await import("../pending");
const { parseCreditAmount } = await import("../../catalog/presentation");

test("代理上限预填为以 K 为单位的纯数字，并仍按 K 解析", () => {
  expect(initialProxyAmount(61_500)).toBe("62");
  expect(initialProxyAmount(1_005_000)).toBe("1005");
  expect(initialProxyAmount(0)).toBe("");
  expect(parseCreditAmount(initialProxyAmount(61_500))).toBe(62_000);
  expect(parseCreditAmount("62")).toBe(62_000);
  expect(parseCreditAmount("1.5M")).toBe(1_500_000);
});

test("代理上限输入仍拒绝非法写法", () => {
  expect(parseCreditAmount("62.5")).toBeNull();
  expect(parseCreditAmount("1.0005M")).toBeNull();
});

test("initial_mode_follows_the_frozen_default：初始模式跟随冻结默认值", () => {
  expect(initialProxyMode("slow", true)).toBe("slow");
  expect(initialProxyMode("slow", false)).toBe("fast");
  expect(initialProxyMode("fast", true)).toBe("fast");
  expect(initialProxyMode("fast", false)).toBe("fast");
  expect(initialProxyMode(null, true)).toBeNull();
  expect(initialProxyMode("Slow", true)).toBeNull();
  expect(initialProxyMode("", true)).toBeNull();
});
