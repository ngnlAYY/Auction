// 平铺子集加载器与 i18n 门禁共用样本的边界回归；两边对同一份文本必须给出相同结论。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import { buildBundle } from "./resource";

type Sample = { name: string; text: string; valid: boolean };

const samples = JSON.parse(
  readFileSync(
    new URL("../../../../../ci/checks/i18n-subset-samples.json", import.meta.url),
    "utf8",
  ),
) as Sample[];

test("平铺子集加载器接受的行形态与 i18n 门禁样本一致", () => {
  for (const sample of samples) {
    expect(buildBundle("en", sample.text, "en", null).ok, sample.name).toBe(sample.valid);
  }
});

test.each(['key = text {"a\\{b"}\n', 'key = text {"a\\}b"}\n'])(
  "平铺子集加载器拒绝 Fluent 不支持的花括号转义 %s",
  (text) => {
    expect(buildBundle("en", text, "en", null).ok).toBe(false);
  },
);
