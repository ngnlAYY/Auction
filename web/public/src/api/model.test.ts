// 目录展示行 lot 代码的回归断言。

import { expect, test } from "bun:test";

const { lotCodeFor } = await import("./model");

test("lot_codes_match_forum_prefixes_and_fall_back_to_lot：lot 代码使用论坛前缀并回退到 Lot", () => {
  expect(lotCodeFor("One-Handed", 1)).toBe("One01");
  expect(lotCodeFor("Cloth", 16)).toBe("Clo16");
  expect(lotCodeFor("Heavy", 120)).toBe("Hea120");
  expect(lotCodeFor("Unknown", 3)).toBe("Lot03");
});
