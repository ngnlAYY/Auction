// 装备属性值的基础值与 roll 百分比格式。

import { expect, test } from "bun:test";

const { statValueLabel } = await import("./stat-format");

test("stat_values_carry_base_and_roll_percent_in_parentheses：属性值在括号里带基础值与 roll 百分比", () => {
  expect(statValueLabel("+109.1", 195.0, 97.5)).toBe("+109.1(195/97.5%)");
  expect(statValueLabel("+1985", 200.0, 100.0)).toBe("+1985(200/100%)");
  // 基础值不是整数时原样保留小数；百分比只保留一位小数。
  expect(statValueLabel("+0.35", 179.5, 89.75)).toBe("+0.35(179.5/89.8%)");
  // 只有其中一项时括号里只放那一项，不留下悬空的斜杠。
  expect(statValueLabel("+0.25", 180.0, null)).toBe("+0.25(180)");
  expect(statValueLabel("+0.25", null, 90.0)).toBe("+0.25(90%)");
  // 没有 roll 的属性只显示数值，不出现空括号。
  expect(statValueLabel("+28.75", null, null)).toBe("+28.75");
});
