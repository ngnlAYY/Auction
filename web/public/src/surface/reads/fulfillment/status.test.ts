// 履约状态闭集到译文键与语气样式的一一映射。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { compareText } from "@auction/web-shared/text";

const [{ FULFILLMENT_STATUSES }, { statusKey, statusTone }] = await Promise.all([
  import("../../../api/decoder/fulfillment"),
  import("./status"),
]);

/** 英文译文在仓库内嵌，其余语言的键集合由 i18n 门禁核对与英文一致。 */
const ENGLISH = readFileSync(
  new URL("../../../../../../crates/auction-web-public/i18n/en.ftl", import.meta.url),
  "utf8",
);

test("every_status_maps_to_its_own_translated_label：每个状态映射到自己的译文", () => {
  const keys: string[] = [];
  for (const status of FULFILLMENT_STATUSES) {
    const key = statusKey(status);
    expect(key).toBe(`fulfillment-status-${status.replaceAll("_", "-")}`);
    expect(
      ENGLISH.split(/\r?\n/).some((line) => {
        const separator = line.indexOf(" =");
        return separator !== -1 && line.slice(0, separator) === key;
      }),
    ).toBe(true);
    keys.push(key);
  }
  keys.sort(compareText);
  expect(new Set(keys).size).toBe(FULFILLMENT_STATUSES.length);
  expect(statusKey("future_status")).toBe("fulfillment-status-unknown");
});

test("every_status_has_a_tone_group：每个状态都有语气分组", () => {
  for (const status of FULFILLMENT_STATUSES) {
    const tone = statusTone(status);
    expect(tone.startsWith("public-fulfillment__status public-fulfillment__status--")).toBe(true);
    // 只有“未知”本身落到未知分组；其余状态漏配会在这里暴露。
    expect(tone.endsWith("--unknown")).toBe(status === "unknown");
  }
  expect(statusTone("completed").endsWith("--done")).toBe(true);
  expect(statusTone("payment_overdue").endsWith("--attention")).toBe(true);
  expect(statusTone("awaiting_payment").endsWith("--progress")).toBe(true);
  for (const status of FULFILLMENT_STATUSES.filter((value) => value.startsWith("voided_"))) {
    expect(statusTone(status).endsWith("--voided")).toBe(true);
  }
});
